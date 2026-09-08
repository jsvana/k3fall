//! Carrier Wave RemoteOp wire frames — Rust mirror of CarrierWaveCore's
//! `RemoteOp` Codable namespace (protocol v1). Frames travel as
//! newline-delimited JSON over TCP. Swift's synthesized enum Codable
//! produces `{"caseName":{"_0":{...payload...}}}`, reproduced here with an
//! externally-tagged enum whose variants wrap payloads in `W<T>` (a struct
//! with a single `_0` field). Optionals are omitted when absent; dates are
//! ISO8601 with fractional seconds; binary payloads are base64.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: i64 = 1;
pub const BONJOUR_SERVICE_TYPE: &str = "_cwsweep-remote._tcp";

/// Swift associated-value wrapper: `{"_0": ...}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct W<T> {
    #[serde(rename = "_0")]
    pub inner: T,
}

impl<T> W<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

pub fn now_iso8601() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[allow(clippy::enum_variant_names)]
pub enum Frame {
    #[serde(rename = "hello")]
    Hello(W<Hello>),
    #[serde(rename = "helloAck")]
    HelloAck(W<HelloAck>),
    #[serde(rename = "pair")]
    Pair(W<Pair>),
    #[serde(rename = "paired")]
    Paired(W<Paired>),
    #[serde(rename = "ping")]
    Ping(W<Ping>),
    #[serde(rename = "pong")]
    Pong(W<Ping>),
    #[serde(rename = "subscribe")]
    Subscribe(W<Subscribe>),
    #[serde(rename = "unsubscribe")]
    Unsubscribe(W<Unsubscribe>),
    #[serde(rename = "eventRadioState")]
    EventRadioState(W<RadioStateEvent>),
    #[serde(rename = "eventSpot")]
    EventSpot(W<SpotEvent>),
    #[serde(rename = "eventSpotsCleared")]
    EventSpotsCleared(W<Empty>),
    #[serde(rename = "commandTune")]
    CommandTune(W<TuneCommand>),
    #[serde(rename = "commandSendCW")]
    CommandSendCw(W<SendCwCommand>),
    #[serde(rename = "commandStopCW")]
    CommandStopCw(W<StopCwCommand>),
    #[serde(rename = "commandSetCWSpeed")]
    CommandSetCwSpeed(W<SetCwSpeedCommand>),
    #[serde(rename = "eventCWStatus")]
    EventCwStatus(W<CwStatusEvent>),
    #[serde(rename = "eventCWSlots")]
    EventCwSlots(W<CwSlotsEvent>),
    #[serde(rename = "eventActiveSession")]
    EventActiveSession(W<ActiveSessionEvent>),
    #[serde(rename = "eventQueriesList")]
    EventQueriesList(W<QueriesListEvent>),
    #[serde(rename = "eventWaterfallLine")]
    EventWaterfallLine(W<WaterfallLineEvent>),
    #[serde(rename = "eventWaterfallCleared")]
    EventWaterfallCleared(W<WaterfallClearedEvent>),
    #[serde(rename = "ack")]
    Ack(W<Ack>),
    #[serde(rename = "errorFrame")]
    ErrorFrame(W<ErrorFrame>),
}

impl Frame {
    pub fn encode_line(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn decode_line(line: &str) -> anyhow::Result<Frame> {
        Ok(serde_json::from_str(line)?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Empty {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub protocol_version: i64,
    pub client_name: String,
    pub client_platform: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HelloAck {
    pub protocol_version: i64,
    pub server_name: String,
    pub server_platform: String,
    pub server_version: String,
    pub authenticated: bool,
    #[serde(rename = "serverTimeUTC")]
    pub server_time_utc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Pair {
    pub code: String,
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Paired {
    pub token: String,
    #[serde(rename = "deviceID")]
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Ping {
    pub nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Subscribe {
    pub topic: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Unsubscribe {
    pub topic: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RadioStateEvent {
    #[serde(rename = "radioID")]
    pub radio_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radio_name: Option<String>,
    pub freq_hz: u64,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vfo: Option<String>,
    pub is_transmitting: bool,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpotEvent {
    pub spot: RemoteSpot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSpot {
    pub id: String,
    pub callsign: String,
    pub freq_hz: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub band: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snr: Option<i32>,
    pub needed: bool,
    pub worked_before: bool,
    pub spotted_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TuneCommand {
    #[serde(rename = "requestID")]
    pub request_id: String,
    #[serde(rename = "radioID", skip_serializing_if = "Option::is_none")]
    pub radio_id: Option<String>,
    pub freq_hz: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SendCwCommand {
    #[serde(rename = "requestID")]
    pub request_id: String,
    pub text: String,
    #[serde(rename = "radioID", skip_serializing_if = "Option::is_none")]
    pub radio_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StopCwCommand {
    #[serde(rename = "requestID")]
    pub request_id: String,
    #[serde(rename = "radioID", skip_serializing_if = "Option::is_none")]
    pub radio_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SetCwSpeedCommand {
    #[serde(rename = "requestID")]
    pub request_id: String,
    pub wpm: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CwStatusEvent {
    #[serde(rename = "radioID")]
    pub radio_id: String,
    pub keyer_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wpm: Option<u8>,
    pub buffer_depth: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sending_slot: Option<u8>,
    pub is_sending: bool,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CwSlotsEvent {
    pub slots: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActiveSessionEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<serde_json::Value>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueriesListEvent {
    pub queries: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WaterfallLineEvent {
    #[serde(rename = "radioID")]
    pub radio_id: String,
    pub source: String,
    pub center_hz: u64,
    pub span_hz: u64,
    pub bin_count: u16,
    /// base64-encoded u8 magnitudes (0 = floor, 255 = peak)
    pub bins: String,
    pub sequence: u64,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WaterfallClearedEvent {
    #[serde(rename = "radioID")]
    pub radio_id: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ack {
    #[serde(rename = "requestID")]
    pub request_id: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ErrorFrame {
    pub code: String,
    pub message: String,
    #[serde(rename = "requestID", skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden strings captured from CarrierWaveCore's RemoteOpFraming
    /// encoder (see docs: swift golden harness). Compared as
    /// serde_json::Value so key order differences are irrelevant.
    fn assert_matches_golden(frame: &Frame, golden: &str) {
        let ours: serde_json::Value = serde_json::from_str(&frame.encode_line().unwrap()).unwrap();
        let theirs: serde_json::Value = serde_json::from_str(golden).unwrap();
        assert_eq!(ours, theirs);
        // And we must be able to decode Swift's encoding.
        let decoded = Frame::decode_line(golden).unwrap();
        assert_eq!(&decoded, frame);
    }

    #[test]
    fn hello_matches_swift() {
        let frame = Frame::Hello(W::new(Hello {
            protocol_version: 1,
            client_name: "Field".into(),
            client_platform: "iOS".into(),
            token: Some("tok123".into()),
        }));
        assert_matches_golden(
            &frame,
            r#"{"hello":{"_0":{"protocolVersion":1,"clientPlatform":"iOS","token":"tok123","clientName":"Field"}}}"#,
        );
    }

    #[test]
    fn hello_ack_matches_swift() {
        let frame = Frame::HelloAck(W::new(HelloAck {
            protocol_version: 1,
            server_name: "k3fall".into(),
            server_platform: "linux".into(),
            server_version: "0.1.0".into(),
            authenticated: true,
            server_time_utc: "2023-11-14T22:13:20.500Z".into(),
        }));
        assert_matches_golden(
            &frame,
            r#"{"helloAck":{"_0":{"serverVersion":"0.1.0","protocolVersion":1,"serverName":"k3fall","serverPlatform":"linux","authenticated":true,"serverTimeUTC":"2023-11-14T22:13:20.500Z"}}}"#,
        );
    }

    #[test]
    fn pair_and_paired_match_swift() {
        assert_matches_golden(
            &Frame::Pair(W::new(Pair {
                code: "123456".into(),
                device_name: "Jay's iPhone".into(),
            })),
            r#"{"pair":{"_0":{"deviceName":"Jay's iPhone","code":"123456"}}}"#,
        );
        assert_matches_golden(
            &Frame::Paired(W::new(Paired {
                token: "tok123".into(),
                device_id: "11111111-2222-3333-4444-555555555555".into(),
            })),
            r#"{"paired":{"_0":{"token":"tok123","deviceID":"11111111-2222-3333-4444-555555555555"}}}"#,
        );
    }

    #[test]
    fn subscribe_matches_swift() {
        assert_matches_golden(
            &Frame::Subscribe(W::new(Subscribe {
                topic: "radio.waterfall".into(),
                filter: None,
            })),
            r#"{"subscribe":{"_0":{"topic":"radio.waterfall"}}}"#,
        );
    }

    #[test]
    fn radio_state_matches_swift() {
        let frame = Frame::EventRadioState(W::new(RadioStateEvent {
            radio_id: "k3fall".into(),
            radio_name: Some("K3".into()),
            freq_hz: 14_032_500,
            mode: "CW".into(),
            vfo: Some("A".into()),
            is_transmitting: false,
            timestamp: "2023-11-14T22:13:20.500Z".into(),
        }));
        assert_matches_golden(
            &frame,
            r#"{"eventRadioState":{"_0":{"vfo":"A","radioName":"K3","isTransmitting":false,"radioID":"k3fall","timestamp":"2023-11-14T22:13:20.500Z","freqHz":14032500,"mode":"CW"}}}"#,
        );
    }

    #[test]
    fn waterfall_line_matches_swift() {
        let frame = Frame::EventWaterfallLine(W::new(WaterfallLineEvent {
            radio_id: "k3fall".into(),
            source: "k3fall".into(),
            center_hz: 14_032_500,
            span_hz: 250_000,
            bin_count: 4,
            bins: "AID/QA==".into(),
            sequence: 42,
            timestamp: "2023-11-14T22:13:20.500Z".into(),
        }));
        assert_matches_golden(
            &frame,
            r#"{"eventWaterfallLine":{"_0":{"sequence":42,"source":"k3fall","radioID":"k3fall","spanHz":250000,"timestamp":"2023-11-14T22:13:20.500Z","centerHz":14032500,"binCount":4,"bins":"AID/QA=="}}}"#,
        );
    }

    #[test]
    fn tune_and_ack_match_swift() {
        assert_matches_golden(
            &Frame::CommandTune(W::new(TuneCommand {
                request_id: "11111111-2222-3333-4444-555555555555".into(),
                radio_id: None,
                freq_hz: 14_040_000,
                mode: None,
            })),
            r#"{"commandTune":{"_0":{"freqHz":14040000,"requestID":"11111111-2222-3333-4444-555555555555"}}}"#,
        );
        assert_matches_golden(
            &Frame::Ack(W::new(Ack {
                request_id: "11111111-2222-3333-4444-555555555555".into(),
                success: true,
                detail: None,
            })),
            r#"{"ack":{"_0":{"success":true,"requestID":"11111111-2222-3333-4444-555555555555"}}}"#,
        );
    }

    #[test]
    fn spot_matches_swift() {
        let frame = Frame::EventSpot(W::new(SpotEvent {
            spot: RemoteSpot {
                id: "s1".into(),
                callsign: "F6HKA".into(),
                freq_hz: 14_008_100,
                mode: Some("CW".into()),
                band: Some("20m".into()),
                source: "RBN".into(),
                snr: Some(22),
                needed: false,
                worked_before: false,
                spotted_at: "2023-11-14T22:13:20.500Z".into(),
                comment: None,
            },
        }));
        assert_matches_golden(
            &frame,
            r#"{"eventSpot":{"_0":{"spot":{"needed":false,"source":"RBN","spottedAt":"2023-11-14T22:13:20.500Z","band":"20m","id":"s1","workedBefore":false,"callsign":"F6HKA","snr":22,"freqHz":14008100,"mode":"CW"}}}}"#,
        );
    }

    #[test]
    fn error_frame_matches_swift() {
        assert_matches_golden(
            &Frame::ErrorFrame(W::new(ErrorFrame {
                code: "not_authenticated".into(),
                message: "pair first".into(),
                request_id: None,
            })),
            r#"{"errorFrame":{"_0":{"message":"pair first","code":"not_authenticated"}}}"#,
        );
    }
}
