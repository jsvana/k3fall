//! Native Carrier Wave RemoteOp server: lets the iOS app ("Field") pair
//! directly with the Pi and stream the waterfall / rig state / spots, and
//! send tune + CW commands — no Mac in the loop. Wire protocol mirrors CW
//! Sweep's server (newline-delimited JSON frames over TCP, Bonjour
//! `_cwsweep-remote._tcp` advertised via avahi on the host).

pub mod frames;

use crate::audio::AudioPacket;
use crate::cat::{direction_for, RigState};
use crate::config::Config;
use crate::dsp::SpectrumRow;
use crate::protocol::Spot;
use crate::shaping::shape_row;
use base64::Engine;
use frames::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, watch};

const PAIR_CODE_TTL: Duration = Duration::from_secs(120);
const WATERFALL_FPS_INTERVAL: Duration = Duration::from_millis(100);
const REMOTE_WATERFALL_BINS: usize = 1024;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    pub token: String,
    pub paired_at: String,
}

/// Shared pairing/registry state; the web UI generates codes into it.
pub struct RemoteState {
    paired: Mutex<Vec<PairedDevice>>,
    pairing: Mutex<Option<(String, Instant)>>,
    state_file: std::path::PathBuf,
}

impl RemoteState {
    pub fn load(state_file: std::path::PathBuf) -> Arc<Self> {
        let paired = std::fs::read_to_string(&state_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Arc::new(Self {
            paired: Mutex::new(paired),
            pairing: Mutex::new(None),
            state_file,
        })
    }

    /// Begin pairing: returns a fresh 6-digit code valid for two minutes.
    pub fn begin_pairing(&self) -> String {
        let code = format!("{:06}", rand_code());
        *self.pairing.lock().unwrap() = Some((code.clone(), Instant::now()));
        code
    }

    fn consume_pairing(&self, code: &str, device_name: &str) -> Option<PairedDevice> {
        let mut slot = self.pairing.lock().unwrap();
        match slot.as_ref() {
            Some((active, at)) if active == code && at.elapsed() < PAIR_CODE_TTL => {}
            _ => return None,
        }
        *slot = None;
        let device = PairedDevice {
            id: uuid_v4(),
            name: device_name.to_string(),
            token: format!("{}{}", uuid_v4_simple(), uuid_v4_simple()),
            paired_at: now_iso8601(),
        };
        let mut paired = self.paired.lock().unwrap();
        paired.push(device.clone());
        let json = serde_json::to_string_pretty(&*paired).unwrap_or_default();
        if let Err(err) = std::fs::write(&self.state_file, json) {
            tracing::warn!(?err, "remoteop: failed to persist paired devices");
        }
        Some(device)
    }

    fn device_for_token(&self, token: &str) -> Option<PairedDevice> {
        self.paired
            .lock()
            .unwrap()
            .iter()
            .find(|d| d.token == token)
            .cloned()
    }
}

fn rand_code() -> u32 {
    // uuid v4 as an entropy source avoids adding a rand dependency.
    let id = uuid::Uuid::new_v4();
    u32::from_le_bytes(id.as_bytes()[0..4].try_into().unwrap()) % 1_000_000
}

fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string().to_uppercase()
}

fn uuid_v4_simple() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub struct Deps {
    pub cfg: Arc<Config>,
    pub rows: broadcast::Sender<SpectrumRow>,
    pub rig: watch::Receiver<RigState>,
    pub spots: watch::Receiver<Vec<Spot>>,
    pub tune: mpsc::Sender<u64>,
    pub cat_raw: mpsc::Sender<String>,
    pub state: Arc<RemoteState>,
    pub audio: broadcast::Sender<AudioPacket>,
    pub audio_enabled: bool,
}

pub async fn run_server(listen: String, deps: Arc<Deps>) {
    let listener = match TcpListener::bind(&listen).await {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!(?err, listen, "remoteop: bind failed");
            return;
        }
    };
    tracing::info!(listen, "remoteop: listening");
    run_server_on(listener, deps).await;
}

pub async fn run_server_on(listener: TcpListener, deps: Arc<Deps>) {
    loop {
        match listener.accept().await {
            Ok((sock, peer)) => {
                tracing::info!(%peer, "remoteop: connection");
                let deps = deps.clone();
                tokio::spawn(async move {
                    if let Err(err) = handle_conn(sock, deps).await {
                        tracing::debug!(?err, "remoteop: connection ended");
                    }
                });
            }
            Err(err) => {
                tracing::warn!(?err, "remoteop: accept failed");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

fn radio_state_event(rig: &RigState, cfg: &Config) -> Frame {
    let freq = if rig.connected { rig.vfo_a_hz } else { 0 };
    Frame::EventRadioState(W::new(RadioStateEvent {
        radio_id: "k3fall".into(),
        radio_name: Some("K3 (k3fall)".into()),
        freq_hz: if freq == 0 { cfg.sdr.center_hz } else { freq },
        mode: rig.mode.clone(),
        vfo: Some("A".into()),
        is_transmitting: false,
        timestamp: now_iso8601(),
    }))
}

fn cw_status_event(rig: &RigState) -> Frame {
    Frame::EventCwStatus(W::new(CwStatusEvent {
        radio_id: "k3fall".into(),
        keyer_available: rig.connected,
        wpm: None,
        buffer_depth: 0,
        sending_slot: None,
        is_sending: false,
        timestamp: now_iso8601(),
    }))
}

fn spot_event(spot: &Spot, cfg: &Config) -> Frame {
    let band = cfg
        .bands
        .iter()
        .find(|b| spot.freq_hz >= b.low_hz && spot.freq_hz <= b.high_hz)
        .map(|b| b.name.clone());
    let spotted_at = chrono::Utc::now() - chrono::Duration::seconds(i64::from(spot.age_s));
    Frame::EventSpot(W::new(SpotEvent {
        spot: RemoteSpot {
            id: format!("{}@{}", spot.callsign, spot.freq_hz),
            callsign: spot.callsign.clone(),
            freq_hz: spot.freq_hz,
            mode: None,
            band,
            source: spot.source.to_uppercase(),
            snr: spot.snr_db,
            needed: false,
            worked_before: false,
            spotted_at: spotted_at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            comment: spot.label.clone(),
        },
    }))
}

/// Split CW text into K3 `KY` commands (max 24 chars of text each).
fn ky_commands(text: &str) -> Vec<String> {
    text.as_bytes()
        .chunks(24)
        .map(|chunk| format!("KY {};", String::from_utf8_lossy(chunk)))
        .collect()
}

async fn handle_conn(sock: tokio::net::TcpStream, deps: Arc<Deps>) -> anyhow::Result<()> {
    let (read_half, mut write) = sock.into_split();
    let mut lines = BufReader::new(read_half).lines();

    let mut authed = false;
    let mut subs: HashSet<String> = HashSet::new();
    let mut rows_rx = deps.rows.subscribe();
    let mut audio_rx = deps.audio.subscribe();
    let mut rig_rx = deps.rig.clone();
    let mut spots_rx = deps.spots.clone();
    let mut sent_spot_ids: HashSet<String> = HashSet::new();
    let mut last_waterfall = Instant::now() - WATERFALL_FPS_INTERVAL;

    macro_rules! send {
        ($frame:expr) => {{
            let mut line = $frame.encode_line()?;
            line.push('\n');
            write.write_all(line.as_bytes()).await?;
        }};
    }

    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { return Ok(()); };
                let frame = match Frame::decode_line(&line) {
                    Ok(frame) => frame,
                    Err(err) => {
                        tracing::debug!(?err, "remoteop: undecodable frame ignored");
                        continue;
                    }
                };
                match frame {
                    Frame::Hello(w) => {
                        let hello = w.inner;
                        if hello.protocol_version != PROTOCOL_VERSION {
                            send!(Frame::ErrorFrame(W::new(ErrorFrame {
                                code: "version_mismatch".into(),
                                message: format!("expected v{PROTOCOL_VERSION}, got v{}", hello.protocol_version),
                                request_id: None,
                            })));
                            return Ok(());
                        }
                        if let Some(token) = hello.token.as_deref() {
                            if deps.state.device_for_token(token).is_some() {
                                authed = true;
                            }
                        }
                        send!(Frame::HelloAck(W::new(HelloAck {
                            protocol_version: PROTOCOL_VERSION,
                            server_name: deps.cfg.remoteop.name.clone(),
                            server_platform: "linux".into(),
                            server_version: env!("CARGO_PKG_VERSION").into(),
                            authenticated: authed,
                            server_time_utc: now_iso8601(),
                        })));
                        if authed {
                            send!(radio_state_event(&rig_rx.borrow().clone(), &deps.cfg));
                        }
                    }
                    Frame::Pair(w) => {
                        match deps.state.consume_pairing(&w.inner.code, &w.inner.device_name) {
                            Some(device) => {
                                authed = true;
                                tracing::info!(name = %device.name, "remoteop: device paired");
                                send!(Frame::Paired(W::new(Paired {
                                    token: device.token,
                                    device_id: device.id,
                                })));
                                send!(radio_state_event(&rig_rx.borrow().clone(), &deps.cfg));
                            }
                            None => {
                                send!(Frame::ErrorFrame(W::new(ErrorFrame {
                                    code: "pairing_refused".into(),
                                    message: "incorrect pairing code".into(),
                                    request_id: None,
                                })));
                            }
                        }
                    }
                    Frame::Ping(w) => send!(Frame::Pong(w)),
                    Frame::Subscribe(w) => {
                        if !authed {
                            send!(Frame::ErrorFrame(W::new(ErrorFrame {
                                code: "not_authenticated".into(),
                                message: "subscribe before pairing/hello-with-token".into(),
                                request_id: None,
                            })));
                            continue;
                        }
                        let topic = w.inner.topic.clone();
                        subs.insert(topic.clone());
                        match topic.as_str() {
                            "radio.state" => send!(radio_state_event(&rig_rx.borrow().clone(), &deps.cfg)),
                            "spots.feed" => {
                                let spots = spots_rx.borrow().clone();
                                for spot in &spots {
                                    let frame = spot_event(spot, &deps.cfg);
                                    if let Frame::EventSpot(ev) = &frame {
                                        sent_spot_ids.insert(ev.inner.spot.id.clone());
                                    }
                                    send!(frame);
                                }
                            }
                            "cw.keyer" => send!(cw_status_event(&rig_rx.borrow().clone())),
                            "cw.slots" => send!(Frame::EventCwSlots(W::new(CwSlotsEvent { slots: vec![] }))),
                            "session.active" => send!(Frame::EventActiveSession(W::new(ActiveSessionEvent {
                                session: None,
                                timestamp: now_iso8601(),
                            }))),
                            "queries.list" => send!(Frame::EventQueriesList(W::new(QueriesListEvent { queries: vec![] }))),
                            "radio.waterfall" => send!(Frame::EventWaterfallCleared(W::new(WaterfallClearedEvent {
                                radio_id: "k3fall".into(),
                                timestamp: now_iso8601(),
                            }))),
                            "radio.audio" => {
                                if deps.audio_enabled {
                                    send!(Frame::EventAudioFormat(W::new(AudioFormatEvent {
                                        radio_id: "k3fall".into(),
                                        source_label: deps.cfg.audio.label.clone(),
                                        codec: "pcm_s16le".into(),
                                        sample_rate: i64::from(deps.cfg.audio.sample_rate),
                                        channels: 1,
                                        bits_per_sample: 16,
                                        frames_per_packet: None,
                                        timestamp: now_iso8601(),
                                    })));
                                }
                            }
                            _ => {}
                        }
                    }
                    Frame::Unsubscribe(w) => {
                        subs.remove(&w.inner.topic);
                    }
                    Frame::CommandTune(w) => {
                        let cmd = w.inner;
                        let ok = authed
                            && rig_rx.borrow().connected
                            && deps.tune.try_send(cmd.freq_hz).is_ok();
                        send!(Frame::Ack(W::new(Ack {
                            request_id: cmd.request_id,
                            success: ok,
                            detail: (!ok).then(|| "tune unavailable".to_string()),
                        })));
                    }
                    Frame::CommandSendCw(w) => {
                        let cmd = w.inner;
                        let mut ok = authed && rig_rx.borrow().connected;
                        if ok {
                            for ky in ky_commands(&cmd.text) {
                                if deps.cat_raw.try_send(ky).is_err() {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        send!(Frame::Ack(W::new(Ack {
                            request_id: cmd.request_id,
                            success: ok,
                            detail: (!ok).then(|| "keyer unavailable".to_string()),
                        })));
                    }
                    Frame::CommandStopCw(w) => {
                        let ok = authed && deps.cat_raw.try_send("RX;".to_string()).is_ok();
                        send!(Frame::Ack(W::new(Ack {
                            request_id: w.inner.request_id,
                            success: ok,
                            detail: None,
                        })));
                    }
                    Frame::CommandSetCwSpeed(w) => {
                        let cmd = w.inner;
                        let ok = authed
                            && deps.cat_raw.try_send(format!("KS{:03};", cmd.wpm.clamp(8, 50))).is_ok();
                        send!(Frame::Ack(W::new(Ack {
                            request_id: cmd.request_id,
                            success: ok,
                            detail: None,
                        })));
                    }
                    _ => {}
                }
            }
            row = rows_rx.recv(), if subs.contains("radio.waterfall") => {
                match row {
                    Ok(row) => {
                        if last_waterfall.elapsed() < WATERFALL_FPS_INTERVAL {
                            continue;
                        }
                        last_waterfall = Instant::now();
                        let rig = rig_rx.borrow().clone();
                        let center = if rig.connected && rig.vfo_a_hz != 0 {
                            rig.vfo_a_hz
                        } else {
                            deps.cfg.sdr.center_hz
                        };
                        let mut shaped = shape_row(&row.bins, 0, row.bins.len(), REMOTE_WATERFALL_BINS);
                        if direction_for(&deps.cfg.bands, center) == -1 {
                            shaped.reverse();
                        }
                        send!(Frame::EventWaterfallLine(W::new(WaterfallLineEvent {
                            radio_id: "k3fall".into(),
                            source: "k3fall".into(),
                            center_hz: center,
                            span_hz: u64::from(deps.cfg.sdr.sample_rate),
                            bin_count: shaped.len() as u16,
                            bins: base64::engine::general_purpose::STANDARD.encode(&shaped),
                            sequence: u64::from(row.seq),
                            timestamp: now_iso8601(),
                        })));
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        rows_rx = rows_rx.resubscribe();
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
            packet = audio_rx.recv(), if subs.contains("radio.audio") && deps.audio_enabled => {
                match packet {
                    Ok(packet) => {
                        send!(Frame::EventAudioFrame(W::new(AudioFrameEvent {
                            radio_id: "k3fall".into(),
                            sequence: packet.seq,
                            samples: base64::engine::general_purpose::STANDARD.encode(&packet.bytes),
                            timestamp: now_iso8601(),
                        })));
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        audio_rx = audio_rx.resubscribe();
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
            changed = rig_rx.changed() => {
                if changed.is_err() { return Ok(()); }
                if authed && subs.contains("radio.state") {
                    send!(radio_state_event(&rig_rx.borrow().clone(), &deps.cfg));
                }
                if authed && subs.contains("cw.keyer") {
                    send!(cw_status_event(&rig_rx.borrow().clone()));
                }
            }
            changed = spots_rx.changed() => {
                if changed.is_err() { return Ok(()); }
                if authed && subs.contains("spots.feed") {
                    let spots = spots_rx.borrow().clone();
                    for spot in &spots {
                        let id = format!("{}@{}", spot.callsign, spot.freq_hz);
                        if sent_spot_ids.insert(id) {
                            send!(spot_event(spot, &deps.cfg));
                        }
                    }
                }
            }
        }
    }
}
