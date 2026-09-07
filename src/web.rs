use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
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
use crate::protocol::{encode_row, ClientMsg, RowFrameHeader, ServerMsg, FLAG_INVERTED};
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
    pub sdr_ok: Arc<AtomicBool>,
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

    let status = ServerMsg::Status {
        sdr_ok: state.sdr_ok.load(Ordering::Relaxed),
        cat_ok: state.rig.borrow().connected,
        fft_size: fft_size as u32,
        bin_hz,
        rows_per_sec: state.cfg.dsp.rows_per_sec,
        center_hz: state.cfg.sdr.center_hz,
        sample_rate,
    };
    if send_json(&mut sender, &status).await.is_err() {
        return;
    }
    if send_rig(&mut sender, &state).await.is_err() {
        return;
    }

    let mut rows_rx = state.rows.subscribe();
    let mut rig_rx = state.rig.clone();
    let mut rig_closed = false;
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
            let connected = state.rig.borrow().connected;
            if !connected {
                let _ = send_json(
                    sender,
                    &ServerMsg::Error {
                        message: "CAT disconnected; cannot tune".to_string(),
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
