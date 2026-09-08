use std::io::{Read, Write};
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::config::{BandConfig, CatConfig};

/// Events produced by the K3 CAT parser as `;`-terminated commands are decoded.
#[derive(Debug, Clone, PartialEq)]
pub enum CatEvent {
    VfoA(u64),
    VfoB(u64),
    Mode(String),
    Other(String),
}

/// Incrementally parses K3 CAT protocol bytes (ASCII, `;`-terminated commands),
/// tolerating partial reads split across multiple `push` calls.
#[derive(Debug, Default)]
pub struct CatParser {
    buf: Vec<u8>,
}

impl CatParser {
    /// Feed newly-read bytes into the parser, returning any fully-parsed events.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<CatEvent> {
        self.buf.extend_from_slice(bytes);

        let mut events = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b';') {
            let cmd_bytes: Vec<u8> = self.buf.drain(..=pos).collect();
            // Drop the trailing ';'.
            let cmd = &cmd_bytes[..cmd_bytes.len() - 1];
            events.push(parse_command(cmd));
        }
        events
    }
}

fn parse_command(cmd: &[u8]) -> CatEvent {
    let raw = String::from_utf8_lossy(cmd).into_owned();

    if let Some(payload) = raw.strip_prefix("FA") {
        return parse_freq(payload, &raw, CatEvent::VfoA);
    }
    if let Some(payload) = raw.strip_prefix("FB") {
        return parse_freq(payload, &raw, CatEvent::VfoB);
    }
    if let Some(payload) = raw.strip_prefix("MD") {
        return parse_mode(payload, &raw);
    }

    CatEvent::Other(raw)
}

fn parse_freq(payload: &str, raw: &str, ctor: impl Fn(u64) -> CatEvent) -> CatEvent {
    if payload.len() == 11 && payload.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(freq) = payload.parse::<u64>() {
            return ctor(freq);
        }
    }
    CatEvent::Other(raw.to_string())
}

fn parse_mode(payload: &str, raw: &str) -> CatEvent {
    let mode = match payload {
        "1" => "LSB",
        "2" => "USB",
        "3" => "CW",
        "4" => "FM",
        "5" => "AM",
        "6" => "DATA",
        "7" => "CW-R",
        "9" => "DATA-R",
        _ => return CatEvent::Other(raw.to_string()),
    };
    CatEvent::Mode(mode.to_string())
}

/// The K3 `FA`/`FB` command format has room for exactly 11 digits of
/// frequency (Hz); this is the largest value that fits.
const MAX_FA_HZ: u64 = 99_999_999_999;

/// Format a K3 `FA` tune command for `freq_hz`, rounded to the nearest 10 Hz.
///
/// Uses `saturating_add` and clamps to `MAX_FA_HZ` before formatting so a
/// pathological/adversarial `freq_hz` (e.g. `u64::MAX`, which would overflow
/// `freq_hz + 5` in debug builds) can't panic this task or produce a
/// malformed (>11-digit) command.
pub fn tune_command(freq_hz: u64) -> String {
    let rounded = (freq_hz.saturating_add(5) / 10 * 10).min(MAX_FA_HZ);
    format!("FA{rounded:011};")
}

/// Look up the configured antenna direction for `freq_hz`, defaulting to `1`
/// when no band matches.
pub fn direction_for(bands: &[BandConfig], freq_hz: u64) -> i8 {
    bands
        .iter()
        .find(|b| freq_hz >= b.low_hz && freq_hz <= b.high_hz)
        .map(|b| b.direction)
        .unwrap_or(1)
}

/// Current view of the rig's state, kept up to date by `run_cat_task`.
#[derive(Debug, Clone)]
pub struct RigState {
    pub vfo_a_hz: u64,
    pub vfo_b_hz: u64,
    pub mode: String,
    pub connected: bool,
}

impl Default for RigState {
    fn default() -> Self {
        RigState {
            vfo_a_hz: 0,
            vfo_b_hz: 0,
            mode: "CW".to_string(),
            connected: false,
        }
    }
}

/// Apply a parsed CAT event to `state`, returning whether anything changed.
fn apply_event(state: &mut RigState, event: &CatEvent) -> bool {
    match event {
        CatEvent::VfoA(hz) => {
            if state.vfo_a_hz != *hz {
                state.vfo_a_hz = *hz;
                true
            } else {
                false
            }
        }
        CatEvent::VfoB(hz) => {
            if state.vfo_b_hz != *hz {
                state.vfo_b_hz = *hz;
                true
            } else {
                false
            }
        }
        CatEvent::Mode(mode) => {
            if &state.mode != mode {
                state.mode = mode.clone();
                true
            } else {
                false
            }
        }
        CatEvent::Other(_) => false,
    }
}

const RECONNECT_BACKOFF: Duration = Duration::from_secs(3);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(2);
const READ_TIMEOUT: Duration = Duration::from_millis(100);
const READ_CHUNK: usize = 256;
/// Consecutive zero-byte reads (distinct from read timeouts) tolerated before
/// a serial port is declared hung up. On POSIX ttys a `read()` returning 0
/// typically means the far end went away; treating a single zero read as fatal
/// risks false positives, so we require a short run of them, pausing between
/// each so a misbehaving driver can't spin a blocking-pool thread at 100%.
const MAX_CONSECUTIVE_ZERO_READS: u32 = 3;
const ZERO_READ_PAUSE: Duration = Duration::from_millis(100);

/// Write `bytes` to the serial port via `spawn_blocking`, since `SerialPort`'s
/// `Write` impl is blocking and must not run directly on an async task.
/// Takes the writer out of `writer_slot` for the duration of the blocking
/// call and always puts it back, so the caller can keep reusing the same
/// `Option` across writes for the life of a connection.
async fn write_blocking(
    writer_slot: &mut Option<Box<dyn serialport::SerialPort>>,
    bytes: Vec<u8>,
) -> std::io::Result<()> {
    let mut w = writer_slot
        .take()
        .expect("writer missing from slot: previous write_blocking call must not have restored it");
    let (w, res) = tokio::task::spawn_blocking(move || {
        let res = w.write_all(&bytes);
        (w, res)
    })
    .await
    .expect("CAT write blocking task panicked");
    *writer_slot = Some(w);
    res
}

/// Wait out `dur` while still observing `tune_rx`. Tune requests that arrive
/// while disconnected (open-retry or reconnect backoff) are logged and
/// dropped rather than queued — they don't shorten or interrupt the backoff.
/// Returns `true` if `tune_rx`'s sender was dropped, meaning the caller
/// should shut `run_cat_task` down entirely instead of retrying.
async fn backoff_or_shutdown(
    dur: Duration,
    tune_rx: &mut mpsc::Receiver<u64>,
    raw_rx: &mut mpsc::Receiver<String>,
    raw_closed: &mut bool,
) -> bool {
    let sleep = tokio::time::sleep(dur);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            _ = &mut sleep => return false,
            raw = raw_rx.recv(), if !*raw_closed => {
                match raw {
                    Some(_) => tracing::debug!("dropping raw CAT command while disconnected"),
                    None => *raw_closed = true,
                }
            }
            msg = tune_rx.recv() => {
                match msg {
                    Some(freq_hz) => {
                        tracing::debug!(freq_hz, "dropping tune request while CAT disconnected");
                    }
                    None => return true,
                }
            }
        }
    }
}

/// Drives a K3 CAT session over `cfg.device`: connects (with reconnect/backoff),
/// reports rig state changes on `state_tx`, forwards tune requests received on
/// `tune_rx` to the rig, and keeps the link alive with periodic `FA;` polls.
///
/// Reads happen in `spawn_blocking` (the `serialport` handle is blocking) with
/// a 100 ms timeout so the task stays responsive to `tune_rx` and shutdown;
/// writes go through `spawn_blocking` too, so a stalled UART can't block the
/// tokio worker thread. Any I/O error other than a read timeout tears down
/// the connection, marks `connected: false`, and retries after a 3 s backoff
/// (which itself stays responsive to `tune_rx`'s sender being dropped, so
/// shutdown during backoff doesn't retry forever).
pub async fn run_cat_task(
    cfg: CatConfig,
    state_tx: watch::Sender<RigState>,
    mut tune_rx: mpsc::Receiver<u64>,
    mut raw_rx: mpsc::Receiver<String>,
) {
    let mut raw_closed = false;
    loop {
        let port = match serialport::new(&cfg.device, cfg.baud)
            .timeout(READ_TIMEOUT)
            .open()
        {
            Ok(port) => port,
            Err(err) => {
                tracing::warn!(device = %cfg.device, error = %err, "failed to open CAT serial port");
                state_tx.send_modify(|s| s.connected = false);
                if backoff_or_shutdown(
                    RECONNECT_BACKOFF,
                    &mut tune_rx,
                    &mut raw_rx,
                    &mut raw_closed,
                )
                .await
                {
                    return;
                }
                continue;
            }
        };

        let writer_box = port;
        let mut reader = match writer_box.try_clone() {
            Ok(clone) => clone,
            Err(err) => {
                tracing::warn!(error = %err, "failed to clone CAT serial handle");
                state_tx.send_modify(|s| s.connected = false);
                if backoff_or_shutdown(
                    RECONNECT_BACKOFF,
                    &mut tune_rx,
                    &mut raw_rx,
                    &mut raw_closed,
                )
                .await
                {
                    return;
                }
                continue;
            }
        };

        let mut writer: Option<Box<dyn serialport::SerialPort>> = Some(writer_box);

        if let Err(err) = write_blocking(&mut writer, b"AI2;FA;FB;MD;".to_vec()).await {
            tracing::warn!(error = %err, "failed to write CAT init command");
            state_tx.send_modify(|s| s.connected = false);
            if backoff_or_shutdown(
                RECONNECT_BACKOFF,
                &mut tune_rx,
                &mut raw_rx,
                &mut raw_closed,
            )
            .await
            {
                return;
            }
            continue;
        }

        state_tx.send_modify(|s| s.connected = true);
        tracing::info!(device = %cfg.device, "CAT serial connected");

        // The reader handle lives entirely on the blocking thread for the life
        // of this connection; chunks (or the terminal error) come back over a
        // channel. This avoids fighting `select!` over ownership of a handle
        // that would otherwise need to move in and out of a fresh blocking
        // task on every loop iteration (unsound: a losing `select!` branch
        // leaves that task still holding the reader on the next iteration).
        let (chunk_tx, mut chunk_rx) = mpsc::channel::<std::io::Result<Vec<u8>>>(16);
        let read_task = tokio::task::spawn_blocking(move || {
            let mut buf = [0u8; READ_CHUNK];
            let mut consecutive_zero_reads = 0u32;
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        consecutive_zero_reads += 1;
                        if consecutive_zero_reads >= MAX_CONSECUTIVE_ZERO_READS {
                            let _ = chunk_tx.blocking_send(Err(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "serial port read returned 0 bytes repeatedly (likely hung up)",
                            )));
                            return;
                        }
                        std::thread::sleep(ZERO_READ_PAUSE);
                    }
                    Ok(n) => {
                        consecutive_zero_reads = 0;
                        if chunk_tx.blocking_send(Ok(buf[..n].to_vec())).is_err() {
                            return;
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::TimedOut => {
                        consecutive_zero_reads = 0;
                    }
                    Err(err) => {
                        let _ = chunk_tx.blocking_send(Err(err));
                        return;
                    }
                }
            }
        });

        let mut parser = CatParser::default();
        let mut local_state = state_tx.borrow().clone();
        let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let disconnect_reason = 'session: loop {
            tokio::select! {
                chunk = chunk_rx.recv() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            let events = parser.push(&bytes);
                            let mut changed = false;
                            for event in &events {
                                if apply_event(&mut local_state, event) {
                                    changed = true;
                                }
                            }
                            if changed {
                                let snapshot = local_state.clone();
                                state_tx.send_modify(|s| *s = snapshot);
                            }
                        }
                        Some(Err(err)) => break 'session format!("read error: {err}"),
                        None => break 'session "read task ended".to_string(),
                    }
                }
                tune = tune_rx.recv() => {
                    match tune {
                        Some(freq_hz) => {
                            let cmd = tune_command(freq_hz);
                            if let Err(err) = write_blocking(&mut writer, cmd.into_bytes()).await {
                                break 'session format!("write error: {err}");
                            }
                        }
                        None => {
                            // Sender dropped: shut the task down entirely.
                            drop(chunk_rx);
                            let _ = read_task.await;
                            return;
                        }
                    }
                }
                raw = raw_rx.recv(), if !raw_closed => {
                    match raw {
                        Some(cmd) => {
                            if let Err(err) = write_blocking(&mut writer, cmd.into_bytes()).await {
                                break 'session format!("write error: {err}");
                            }
                        }
                        None => {
                            // RemoteOp sender gone; the tune channel still
                            // governs task lifetime, so just stop selecting.
                            raw_closed = true;
                        }
                    }
                }
                _ = keepalive.tick() => {
                    if let Err(err) = write_blocking(&mut writer, b"FA;".to_vec()).await {
                        break 'session format!("keepalive write error: {err}");
                    }
                }
            }
        };

        // Dropping the receiver causes the blocking task's next send to fail,
        // so it exits within one read timeout (100 ms).
        drop(chunk_rx);
        let _ = read_task.await;

        tracing::warn!(reason = %disconnect_reason, "CAT serial disconnected");
        state_tx.send_modify(|s| s.connected = false);
        if backoff_or_shutdown(
            RECONNECT_BACKOFF,
            &mut tune_rx,
            &mut raw_rx,
            &mut raw_closed,
        )
        .await
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fa_response() {
        let mut p = CatParser::default();
        let ev = p.push(b"FA00014032500;");
        assert_eq!(ev, vec![CatEvent::VfoA(14_032_500)]);
    }

    #[test]
    fn reassembles_split_input_and_multiple_commands() {
        let mut p = CatParser::default();
        assert!(p.push(b"FA000140").is_empty());
        let ev = p.push(b"32500;MD3;FB00014028000;");
        assert_eq!(
            ev,
            vec![
                CatEvent::VfoA(14_032_500),
                CatEvent::Mode("CW".into()),
                CatEvent::VfoB(14_028_000),
            ]
        );
    }

    #[test]
    fn tune_command_rounds_to_10hz() {
        assert_eq!(tune_command(14_032_504), "FA00014032500;");
        assert_eq!(tune_command(14_032_505), "FA00014032510;");
    }

    #[test]
    fn tune_command_clamps_pathological_frequency_without_panicking() {
        let cmd = tune_command(u64::MAX);
        assert_eq!(cmd, "FA99999999999;");
        // 11 digits between "FA" and ";" -- never a malformed (wider) command.
        let digits = &cmd[2..cmd.len() - 1];
        assert_eq!(digits.len(), 11);
        assert!(digits.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn direction_lookup() {
        let bands = crate::config::Config::default_dev().bands;
        assert_eq!(direction_for(&bands, 14_032_500), -1);
        assert_eq!(direction_for(&bands, 7_030_000), -1);
        assert_eq!(direction_for(&bands, 5_000_000), 1); // out of band → default
    }
}
