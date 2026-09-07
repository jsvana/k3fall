use futures_util::{SinkExt, StreamExt};
use k3fall::cat::RigState;
use k3fall::config::Config;
use k3fall::dsp::{run_dsp_thread, SpectrumRow};
use k3fall::iq::{tone, FileSource};
use k3fall::protocol::{decode_row, ServerMsg};
use k3fall::web::{router, AppState};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, watch};
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// End-to-end smoke test: spin up the real router with a DSP thread driven by
/// a synthesized tone, connect over a real WebSocket, and confirm the session
/// greeting + view negotiation + shaped binary row all work together.
#[tokio::test]
async fn websocket_streams_status_then_shaped_row() {
    let cfg = Arc::new(Config::default_dev());
    let sample_rate = cfg.sdr.sample_rate;

    // A single tone is enough to drive the DSP pipeline; we only care that
    // rows keep flowing, not about spectral content here.
    let samples = tone(sample_rate, 10_000.0, 0.2, sample_rate as usize * 2);

    let (rows_tx, _rows_rx) = broadcast::channel::<SpectrumRow>(32);
    let (_rig_tx, rig_rx) = watch::channel(RigState::default());
    let (tune_tx, _tune_rx) = mpsc::channel::<u64>(8);
    let sdr_ok = Arc::new(AtomicBool::new(true));

    {
        let rows_tx = rows_tx.clone();
        let dsp_cfg = cfg.dsp.clone();
        let sdr_ok = sdr_ok.clone();
        // The reconnect loop only calls this closure once here since the
        // in-memory tone source never errors; pace=true keeps the thread
        // from flooding the broadcast channel faster than the test can keep
        // up with.
        let make_source: Box<dyn FnMut() -> anyhow::Result<Box<dyn k3fall::iq::IqSource>> + Send> =
            Box::new(move || {
                Ok(
                    Box::new(FileSource::from_samples(samples.clone(), sample_rate))
                        as Box<dyn k3fall::iq::IqSource>,
                )
            });
        std::thread::spawn(move || {
            run_dsp_thread(make_source, dsp_cfg, rows_tx, true, sdr_ok);
        });
    }

    let state = AppState {
        rows: rows_tx,
        rig: rig_rx,
        tune: tune_tx,
        cfg: cfg.clone(),
        sdr_ok,
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let url = format!("ws://{addr}/ws");
    let (mut ws_stream, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    // First message must be the JSON Status frame.
    let first = tokio::time::timeout(Duration::from_secs(2), ws_stream.next())
        .await
        .expect("timed out waiting for first message")
        .expect("stream ended before first message")
        .expect("websocket error on first message");
    let text = match first {
        WsMessage::Text(t) => t,
        other => panic!("expected text status message, got {other:?}"),
    };
    let status: ServerMsg = serde_json::from_str(&text).expect("status message is valid JSON");
    match status {
        ServerMsg::Status { .. } => {}
        other => panic!("expected ServerMsg::Status, got {other:?}"),
    }

    // Negotiate a 512-wide view.
    ws_stream
        .send(WsMessage::Text(
            r#"{"type":"view","width":512,"span_hz":250000,"center_offset_hz":0}"#.to_string(),
        ))
        .await
        .unwrap();

    // Within 2s we should see a binary row frame that decodes with 512 bins
    // (other JSON messages, e.g. the initial Rig frame, may arrive first).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut got_row = false;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let next = match tokio::time::timeout(remaining, ws_stream.next()).await {
            Ok(Some(Ok(msg))) => msg,
            _ => break,
        };
        if let WsMessage::Binary(bytes) = next {
            let (header, payload) = decode_row(&bytes).expect("row frame decodes");
            // The default 1024-wide view can race a binary frame in before
            // our view negotiation lands server-side; skip those and keep
            // waiting for the 512-bin frame we asked for.
            if header.bins as usize != 512 {
                continue;
            }
            assert_eq!(payload.len(), 512);
            got_row = true;
            break;
        }
    }
    assert!(got_row, "did not receive a shaped binary row within 2s");
}
