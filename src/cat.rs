#![allow(dead_code)]

use crate::config::BandConfig;

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

/// Format a K3 `FA` tune command for `freq_hz`, rounded to the nearest 10 Hz.
pub fn tune_command(freq_hz: u64) -> String {
    let rounded = (freq_hz + 5) / 10 * 10;
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
    fn direction_lookup() {
        let bands = crate::config::Config::default_dev().bands;
        assert_eq!(direction_for(&bands, 14_032_500), 1);
        assert_eq!(direction_for(&bands, 7_030_000), -1);
        assert_eq!(direction_for(&bands, 5_000_000), 1); // out of band → default
    }
}
