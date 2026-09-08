use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::future::poll_fn;
use futures_util::stream::SplitSink;
use futures_util::{FutureExt, Sink, SinkExt, StreamExt};
use tokio::sync::{broadcast, mpsc, watch};

use crate::cat::{direction_for, RigState};
use crate::config::Config;
use crate::dsp::SpectrumRow;
use crate::protocol::{encode_row, ClientMsg, RowFrameHeader, ServerMsg, Spot, FLAG_INVERTED};
use crate::shaping::shape_row;

/// Maximum number of `Tune` requests accepted from a single client per
/// second; excess requests inside the window are dropped silently.
const TUNE_RATE_LIMIT: usize = 10;
const TUNE_RATE_WINDOW: Duration = Duration::from_secs(1);

/// Shared state handed to every websocket session.
///
/// All fields are cheap to clone (channel handles / `Arc`s), since axum
/// clones `AppState` per connection via the `State` extractor.
#[derive(Clone)]
pub struct AppState {
    pub rows: broadcast::Sender<SpectrumRow>,
    pub rig: watch::Receiver<RigState>,
    pub tune: mpsc::Sender<u64>,
    pub cfg: Arc<Config>,
    /// Live SDR-open/streaming status. `watch` (rather than a plain flag)
    /// lets active websocket sessions react immediately when it changes
    /// instead of only reporting the value seen at connect time.
    pub sdr_ok_rx: watch::Receiver<bool>,
    /// Latest merged enrichment spots (RBN/POTA) for the current band.
    pub spots_rx: watch::Receiver<Vec<Spot>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/style.css", get(style_css))
        .route("/ws", get(ws_handler))
        .with_state(state)
}

async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../web/index.html"),
    )
}

async fn app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        include_str!("../web/app.js"),
    )
}

async fn style_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css")],
        include_str!("../web/style.css"),
    )
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Client-selected viewport into the spectrum: which slice of the full FFT
/// row (in Hz, relative to the SDR's baseband center) to shape down to
/// `width` output pixels.
#[derive(Debug, Clone)]
struct View {
    width: u16,
    span_hz: u32,
    center_offset_hz: i64,
}

impl View {
    fn new(sample_rate: u32) -> Self {
        View {
            width: 1024,
            span_hz: sample_rate,
            center_offset_hz: 0,
        }
    }

    /// Apply a client-requested view, clamping every field so the resulting
    /// window always stays inside the available spectrum.
    fn apply(
        &mut self,
        width: u16,
        span_hz: u32,
        center_offset_hz: i64,
        sample_rate: u32,
        min_span_hz: u32,
    ) {
        self.width = width.clamp(16, 4096);

        let min_span = min_span_hz.max(1);
        let max_span = sample_rate.max(min_span);
        self.span_hz = span_hz.clamp(min_span, max_span);

        let half_total = (sample_rate / 2) as i64;
        let half_span = (self.span_hz / 2) as i64;
        let max_offset = (half_total - half_span).max(0);
        self.center_offset_hz = center_offset_hz.clamp(-max_offset, max_offset);
    }

    /// Convert the current view into `[start, end)` bin indices over a full
    /// fft-shifted row (index 0 = -sample_rate/2, index fft_size/2 = DC).
    fn bin_range(&self, sample_rate: u32, fft_size: usize, bin_hz: f32) -> (usize, usize) {
        let half_total_hz = sample_rate as f32 / 2.0;
        let start_hz = self.center_offset_hz as f32 - self.span_hz as f32 / 2.0;
        let end_hz = self.center_offset_hz as f32 + self.span_hz as f32 / 2.0;

        let max_bin = fft_size as i64;
        let start_bin = (((start_hz + half_total_hz) / bin_hz).round() as i64).clamp(0, max_bin);
        let end_bin = (((end_hz + half_total_hz) / bin_hz).round() as i64).clamp(0, max_bin);

        let start = start_bin as usize;
        let mut end = end_bin as usize;
        if end <= start {
            end = (start + 1).min(fft_size);
        }
        (start, end)
    }
}

/// RF frequency to report as the row/rig center: the live VFO A frequency
/// when CAT is connected, otherwise the SDR's configured center frequency.
fn effective_center(cfg: &Config, rig: &RigState) -> u64 {
    if rig.connected {
        rig.vfo_a_hz
    } else {
        cfg.sdr.center_hz
    }
}

async fn send_json(
    sender: &mut SplitSink<WebSocket, Message>,
    msg: &ServerMsg,
) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).expect("ServerMsg always serializes to JSON");
    sender.send(Message::Text(text)).await
}

async fn send_rig(
    sender: &mut SplitSink<WebSocket, Message>,
    state: &AppState,
) -> Result<(), axum::Error> {
    let rig = state.rig.borrow().clone();
    let center = effective_center(&state.cfg, &rig);
    let direction = direction_for(&state.cfg.bands, center);
    let msg = ServerMsg::Rig {
        vfo_a_hz: rig.vfo_a_hz,
        vfo_b_hz: rig.vfo_b_hz,
        mode: rig.mode,
        direction,
        cat_connected: rig.connected,
    };
    send_json(sender, &msg).await
}

/// Send a `Status` frame reflecting the current `sdr_ok`/`cat_ok` state.
///
/// Called once at connect and again any time `state.sdr_ok_rx` changes, so
/// clients learn about SDR reconnects/drops without having to reconnect
/// their websocket (finding I3).
async fn send_status(
    sender: &mut SplitSink<WebSocket, Message>,
    state: &AppState,
    sdr_ok: bool,
    fft_size: u32,
    bin_hz: f32,
    sample_rate: u32,
) -> Result<(), axum::Error> {
    let msg = ServerMsg::Status {
        sdr_ok,
        cat_ok: state.rig.borrow().connected,
        fft_size,
        bin_hz,
        rows_per_sec: state.cfg.dsp.rows_per_sec,
        center_hz: state.cfg.sdr.center_hz,
        sample_rate,
    };
    send_json(sender, &msg).await
}

/// Try to push the most recently computed row frame through the socket
/// without blocking the session loop.
///
/// `pending` always holds at most one frame: the newest row we haven't
/// managed to hand to the sink yet. We only call `start_send` (via `feed`)
/// once `ready()` reports the sink can accept it — `tokio-tungstenite`'s
/// `poll_ready` drains any previously buffered-but-unflushed frame first,
/// so gating on `ready()` guarantees we never have more than one frame
/// in flight in the sink plus the one frame held in `pending`. If the sink
/// isn't ready, the frame stays in `pending` and will be overwritten by the
/// next row that arrives before we get another chance to send — i.e. the
/// newest row always wins, older stalled rows are discarded.
///
/// Returns `true` if the connection should be closed due to a fatal sink
/// error.
async fn try_send_pending(
    sender: &mut SplitSink<WebSocket, Message>,
    pending: &mut Option<Vec<u8>>,
) -> bool {
    let Some(frame) = pending.take() else {
        return false;
    };
    let ready = poll_fn(|cx| Pin::new(&mut *sender).poll_ready(cx)).now_or_never();
    match ready {
        Some(Ok(())) => {
            if sender.feed(Message::Binary(frame)).await.is_err() {
                return true;
            }
            // Kick off the actual write; if it can't complete immediately
            // the sink keeps draining it on the next `ready()`/`feed()`
            // call, so we don't need to await it here.
            let _ = sender.flush().now_or_never();
            false
        }
        Some(Err(_)) => true,
        None => {
            // Not ready: leave this frame in the slot so a newer row (if
            // one shows up before we retry) can still overwrite it.
            *pending = Some(frame);
            false
        }
    }
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    let sample_rate = state.cfg.sdr.sample_rate;
    let fft_size = state.cfg.dsp.fft_size;
    let bin_hz = sample_rate as f32 / fft_size as f32;
    // Lower clamp bound for a client-requested span: at least one bin wide.
    let min_span_hz = bin_hz.max(1.0).ceil() as u32;

    let mut sdr_ok_rx = state.sdr_ok_rx.clone();
    let initial_sdr_ok = *sdr_ok_rx.borrow();
    if send_status(
        &mut sender,
        &state,
        initial_sdr_ok,
        fft_size as u32,
        bin_hz,
        sample_rate,
    )
    .await
    .is_err()
    {
        return;
    }
    if send_rig(&mut sender, &state).await.is_err() {
        return;
    }
    let mut spots_rx = state.spots_rx.clone();
    let mut spots_closed = false;
    {
        let spots = spots_rx.borrow().clone();
        if !spots.is_empty()
            && send_json(&mut sender, &ServerMsg::Spots { spots })
                .await
                .is_err()
        {
            return;
        }
    }

    let mut rows_rx = state.rows.subscribe();
    let mut rig_rx = state.rig.clone();
    let mut rig_closed = false;
    let mut sdr_ok_closed = false;
    let mut view = View::new(sample_rate);
    let mut tune_timestamps: VecDeque<Instant> = VecDeque::with_capacity(TUNE_RATE_LIMIT);
    // Holds at most one not-yet-sent row frame: the newest one, so a slow
    // socket drops stale rows instead of a backlog building up.
    let mut pending_row: Option<Vec<u8>> = None;

    loop {
        tokio::select! {
            row = rows_rx.recv() => {
                match row {
                    Ok(row) => {
                        let rig = state.rig.borrow().clone();
                        let center = effective_center(&state.cfg, &rig);
                        let direction = direction_for(&state.cfg.bands, center);
                        let (start, end) = view.bin_range(sample_rate, fft_size, bin_hz);
                        let shaped = shape_row(&row.bins, start, end, view.width as usize);
                        let header = RowFrameHeader {
                            seq: row.seq,
                            center_rf_hz: center,
                            span_hz: view.span_hz,
                            bins: shaped.len() as u16,
                            flags: if direction == -1 { FLAG_INVERTED } else { 0 },
                        };
                        // Newest row always overwrites whatever was still
                        // waiting to go out (explicit discard of any stale
                        // frame so the intent reads clearly).
                        let _ = pending_row.take();
                        pending_row = Some(encode_row(&header, &shaped));
                        if try_send_pending(&mut sender, &mut pending_row).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::debug!(skipped, "row broadcast lagged; resubscribing");
                        rows_rx = rows_rx.resubscribe();
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            changed = rig_rx.changed(), if !rig_closed => {
                match changed {
                    Ok(()) => {
                        if send_rig(&mut sender, &state).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        // Sender side closed; stop selecting on this branch
                        // so we don't spin on an already-resolved future.
                        rig_closed = true;
                    }
                }
            }
            changed = sdr_ok_rx.changed(), if !sdr_ok_closed => {
                match changed {
                    Ok(()) => {
                        let sdr_ok = *sdr_ok_rx.borrow();
                        if send_status(&mut sender, &state, sdr_ok, fft_size as u32, bin_hz, sample_rate)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => {
                        // Sender side closed; stop selecting on this branch
                        // so we don't spin on an already-resolved future.
                        sdr_ok_closed = true;
                    }
                }
            }
            changed = spots_rx.changed(), if !spots_closed => {
                match changed {
                    Ok(()) => {
                        let spots = spots_rx.borrow().clone();
                        if send_json(&mut sender, &ServerMsg::Spots { spots }).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        spots_closed = true;
                    }
                }
            }
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_client_text(
                            &text,
                            &mut sender,
                            &mut view,
                            &mut tune_timestamps,
                            &state,
                            sample_rate,
                            min_span_hz,
                        )
                        .await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {
                        // Ping/Pong/Binary from a client are not part of the
                        // protocol; ignore them.
                    }
                    Some(Err(_)) => break,
                }
            }
        }
    }
}

async fn handle_client_text(
    text: &str,
    sender: &mut SplitSink<WebSocket, Message>,
    view: &mut View,
    tune_timestamps: &mut VecDeque<Instant>,
    state: &AppState,
    sample_rate: u32,
    min_span_hz: u32,
) {
    let parsed: Result<ClientMsg, _> = serde_json::from_str(text);
    match parsed {
        Ok(ClientMsg::View {
            width,
            span_hz,
            center_offset_hz,
        }) => {
            view.apply(width, span_hz, center_offset_hz, sample_rate, min_span_hz);
        }
        Ok(ClientMsg::Tune { freq_hz }) => {
            let rig = state.rig.borrow().clone();
            if !rig.connected {
                let _ = send_json(
                    sender,
                    &ServerMsg::Error {
                        message: "CAT disconnected; cannot tune".to_string(),
                    },
                )
                .await;
                return;
            }

            // Reject requests outside the span currently displayed to this
            // session -- the same center/span used to build the row frames
            // it's been sent (finding I4). Uses saturating arithmetic since
            // `freq_hz` is client-controlled and can be adversarial.
            let center = effective_center(&state.cfg, &rig);
            let half_span = (view.span_hz / 2) as u64;
            let lo = center.saturating_sub(half_span);
            let hi = center.saturating_add(half_span);
            if freq_hz < lo || freq_hz > hi {
                let _ = send_json(
                    sender,
                    &ServerMsg::Error {
                        message: "tune outside displayed span".to_string(),
                    },
                )
                .await;
                return;
            }

            let now = Instant::now();
            while tune_timestamps
                .front()
                .is_some_and(|t| now.duration_since(*t) > TUNE_RATE_WINDOW)
            {
                tune_timestamps.pop_front();
            }
            if tune_timestamps.len() >= TUNE_RATE_LIMIT {
                tracing::debug!(freq_hz, "tune rate limit exceeded; dropping request");
                return;
            }
            tune_timestamps.push_back(now);
            match state.tune.try_send(freq_hz) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::debug!(freq_hz, "tune channel full; dropping request");
                    let _ = send_json(
                        sender,
                        &ServerMsg::Error {
                            message: "tune busy, dropped".to_string(),
                        },
                    )
                    .await;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    tracing::debug!("tune channel closed; CAT task may have shut down");
                }
            }
        }
        Ok(ClientMsg::Display {
            ref_level_db,
            range_db,
        }) => {
            tracing::debug!(
                ref_level_db,
                range_db,
                "display message accepted and ignored (server-side ref/range not applied in v1)"
            );
        }
        Ok(ClientMsg::Power { on }) => {
            let Some(device) = state.cfg.power.device.clone() else {
                let _ = send_json(
                    sender,
                    &ServerMsg::Error {
                        message: "power control not configured ([power] device)".to_string(),
                    },
                )
                .await;
                return;
            };
            let baud = state.cfg.power.baud;
            let result = relay_power(device, baud, on).await;
            if let Err(err) = &result {
                tracing::warn!(?err, "power relay failed");
            }
            let _ = send_json(
                sender,
                &ServerMsg::Power {
                    on,
                    ok: result.is_ok(),
                },
            )
            .await;
        }
        Err(err) => {
            let _ = send_json(
                sender,
                &ServerMsg::Error {
                    message: format!("bad message: {err}"),
                },
            )
            .await;
        }
    }
}

/// Forward a power request to the k3pwr ESP32 over USB serial. Opens the
/// port per command (survives replug, avoids holding it) and waits for the
/// firmware's "ok on"/"ok off" ack, skipping any boot/log chatter.
async fn relay_power(device: String, baud: u32, on: bool) -> anyhow::Result<()> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        use std::io::{BufRead, BufReader, Write};
        let want = if on { "ok on" } else { "ok off" };
        let cmd = if on { "on\n" } else { "off\n" };
        let port = serialport::new(&device, baud)
            .timeout(std::time::Duration::from_millis(500))
            .open()?;
        let mut writer = port.try_clone()?;
        writer.write_all(cmd.as_bytes())?;
        writer.flush()?;
        let mut reader = BufReader::new(port);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut line = String::new();
        while std::time::Instant::now() < deadline {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(_) if line.trim() == want => return Ok(()),
                Ok(_) => continue, // boot banner / log noise / stale acks
                Err(err) if err.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(err) => return Err(err.into()),
            }
        }
        anyhow::bail!("no ack from k3pwr on {device}")
    })
    .await?
}
