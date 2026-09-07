use serde::{Deserialize, Serialize};

pub const FLAG_INVERTED: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowFrameHeader {
    pub seq: u32,
    pub center_rf_hz: u64,
    pub span_hz: u32,
    pub bins: u16,
    pub flags: u8,
}

pub fn encode_row(h: &RowFrameHeader, bins: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(19 + bins.len());
    buf.extend_from_slice(&h.seq.to_le_bytes());
    buf.extend_from_slice(&h.center_rf_hz.to_le_bytes());
    buf.extend_from_slice(&h.span_hz.to_le_bytes());
    buf.extend_from_slice(&h.bins.to_le_bytes());
    buf.push(h.flags);
    buf.extend_from_slice(bins);
    buf
}

pub fn decode_row(buf: &[u8]) -> Option<(RowFrameHeader, &[u8])> {
    if buf.len() < 19 {
        return None;
    }
    let seq = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let center_rf_hz = u64::from_le_bytes([
        buf[4], buf[5], buf[6], buf[7], buf[8], buf[9], buf[10], buf[11],
    ]);
    let span_hz = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
    let bins = u16::from_le_bytes([buf[16], buf[17]]);
    let flags = buf[18];

    let payload = &buf[19..];
    if payload.len() != bins as usize {
        return None;
    }

    Some((
        RowFrameHeader {
            seq,
            center_rf_hz,
            span_hz,
            bins,
            flags,
        },
        payload,
    ))
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    View {
        width: u16,
        span_hz: u32,
        center_offset_hz: i64,
    },
    Tune {
        freq_hz: u64,
    },
    Display {
        ref_level_db: f32,
        range_db: f32,
    },
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Rig {
        vfo_a_hz: u64,
        vfo_b_hz: u64,
        mode: String,
        direction: i8,
        cat_connected: bool,
    },
    Status {
        sdr_ok: bool,
        cat_ok: bool,
        fft_size: u32,
        bin_hz: f32,
        rows_per_sec: u32,
        center_hz: u64,
        sample_rate: u32,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_frame_roundtrip() {
        let h = RowFrameHeader {
            seq: 7,
            center_rf_hz: 14_032_500,
            span_hz: 250_000,
            bins: 3,
            flags: FLAG_INVERTED,
        };
        let buf = encode_row(&h, &[1, 2, 3]);
        assert_eq!(buf.len(), 19 + 3);
        let (h2, payload) = decode_row(&buf).unwrap();
        assert_eq!(h2.seq, 7);
        assert_eq!(h2.center_rf_hz, 14_032_500);
        assert_eq!(payload, &[1, 2, 3]);
    }

    #[test]
    fn client_msg_json_shape() {
        let msg: ClientMsg = serde_json::from_str(r#"{"type":"tune","freq_hz":14032500}"#).unwrap();
        assert_eq!(
            msg,
            ClientMsg::Tune {
                freq_hz: 14_032_500
            }
        );
    }
}
