//! RemoteOp handshake integration: hello -> pair -> subscribe -> tune,
//! exercising the real TCP listener and frame envelopes end to end.

use k3fall::config::Config;
use k3fall::remoteop::{self, frames::Frame, Deps, RemoteState};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

async fn send_line(sock: &mut tokio::net::tcp::OwnedWriteHalf, json: &str) {
    sock.write_all(json.as_bytes()).await.unwrap();
    sock.write_all(b"\n").await.unwrap();
}

#[tokio::test]
async fn pairing_handshake_and_tune() {
    let cfg = Arc::new(Config::default_dev());
    let (rows_tx, _) = tokio::sync::broadcast::channel(8);
    let (rig_tx, rig_rx) = tokio::sync::watch::channel(k3fall::cat::RigState {
        vfo_a_hz: 14_032_500,
        vfo_b_hz: 0,
        mode: "CW".to_string(),
        connected: true,
    });
    let (_spots_tx, spots_rx) = tokio::sync::watch::channel(Vec::new());
    let (tune_tx, mut tune_rx) = tokio::sync::mpsc::channel(4);
    let (cat_raw_tx, mut cat_raw_rx) = tokio::sync::mpsc::channel(4);
    let state = RemoteState::load(
        std::env::temp_dir().join(format!("k3fall-remoteop-test-{}.json", std::process::id())),
    );
    let code = state.begin_pairing();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let deps = Arc::new(Deps {
        cfg,
        rows: rows_tx,
        rig: rig_rx,
        spots: spots_rx,
        tune: tune_tx,
        cat_raw: cat_raw_tx,
        state,
    });
    tokio::spawn(remoteop::run_server_on(listener, deps));
    let _keep_rig = rig_tx;

    let sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    let (read_half, mut write_half) = sock.into_split();
    let mut lines = BufReader::new(read_half).lines();

    // hello without token -> unauthenticated ack
    send_line(
        &mut write_half,
        r#"{"hello":{"_0":{"protocolVersion":1,"clientName":"test","clientPlatform":"iOS"}}}"#,
    )
    .await;
    let ack = Frame::decode_line(&lines.next_line().await.unwrap().unwrap()).unwrap();
    match &ack {
        Frame::HelloAck(w) => assert!(!w.inner.authenticated),
        other => panic!("expected helloAck, got {other:?}"),
    }

    // pair with the active code -> paired + radio state snapshot
    send_line(
        &mut write_half,
        &format!(r#"{{"pair":{{"_0":{{"code":"{code}","deviceName":"test"}}}}}}"#),
    )
    .await;
    let paired = Frame::decode_line(&lines.next_line().await.unwrap().unwrap()).unwrap();
    let token = match &paired {
        Frame::Paired(w) => w.inner.token.clone(),
        other => panic!("expected paired, got {other:?}"),
    };
    assert!(!token.is_empty());
    let snap = Frame::decode_line(&lines.next_line().await.unwrap().unwrap()).unwrap();
    match &snap {
        Frame::EventRadioState(w) => assert_eq!(w.inner.freq_hz, 14_032_500),
        other => panic!("expected radio state, got {other:?}"),
    }

    // subscribe waterfall -> cleared snapshot
    send_line(
        &mut write_half,
        r#"{"subscribe":{"_0":{"topic":"radio.waterfall"}}}"#,
    )
    .await;
    let cleared = Frame::decode_line(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert!(matches!(cleared, Frame::EventWaterfallCleared(_)));

    // tune command -> forwarded + acked
    send_line(
        &mut write_half,
        r#"{"commandTune":{"_0":{"requestID":"11111111-2222-3333-4444-555555555555","freqHz":14040000}}}"#,
    )
    .await;
    let ack = Frame::decode_line(&lines.next_line().await.unwrap().unwrap()).unwrap();
    match &ack {
        Frame::Ack(w) => assert!(w.inner.success),
        other => panic!("expected ack, got {other:?}"),
    }
    assert_eq!(tune_rx.recv().await, Some(14_040_000));

    // sendCW -> KY chunks reach the CAT raw channel
    send_line(
        &mut write_half,
        r#"{"commandSendCW":{"_0":{"requestID":"11111111-2222-3333-4444-555555555555","text":"TU 73"}}}"#,
    )
    .await;
    let ack = Frame::decode_line(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert!(matches!(ack, Frame::Ack(w) if w.inner.success));
    assert_eq!(cat_raw_rx.recv().await.as_deref(), Some("KY TU 73;"));
}
