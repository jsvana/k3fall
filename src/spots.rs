//! Spot enrichment: polls RBN (Vail ReRBN) and POTA for activity on the
//! current band and publishes deduplicated spots to web sessions.

use crate::cat::RigState;
use crate::config::BandConfig;
use crate::protocol::Spot;
use chrono::{DateTime, NaiveDateTime, Utc};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::watch;

const MAX_AGE_SECS: i64 = 600;
const MAX_SPOTS: usize = 50;

/// Band name ("20m") for a frequency, from the config band table.
pub fn band_name_for(bands: &[BandConfig], freq_hz: u64) -> Option<&str> {
    bands
        .iter()
        .find(|b| freq_hz >= b.low_hz && freq_hz <= b.high_hz)
        .map(|b| b.name.as_str())
}

fn parse_iso_utc(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(ts) = DateTime::parse_from_rfc3339(raw) {
        return Some(ts.with_timezone(&Utc));
    }
    // POTA spotTime has no timezone suffix; it is UTC.
    NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

/// Parse the Vail ReRBN /spots response body into spots.
pub fn parse_rbn(body: &serde_json::Value, now: DateTime<Utc>) -> Vec<Spot> {
    let Some(items) = body.get("spots").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let callsign = item.get("callsign")?.as_str()?.to_string();
            let freq_khz = item.get("frequency")?.as_f64()?;
            let age_s = item
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(parse_iso_utc)
                .map(|ts| (now - ts).num_seconds().max(0))
                .unwrap_or(0);
            Some(Spot {
                callsign,
                freq_hz: (freq_khz * 1000.0).round() as u64,
                source: "rbn".to_string(),
                snr_db: item.get("snr").and_then(|v| v.as_i64()).map(|v| v as i32),
                label: None,
                age_s: age_s as u32,
            })
        })
        .collect()
}

/// Parse the POTA /spot/activator response body into spots.
pub fn parse_pota(body: &serde_json::Value, now: DateTime<Utc>) -> Vec<Spot> {
    let Some(items) = body.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let callsign = item.get("activator")?.as_str()?.to_string();
            let freq_khz: f64 = match item.get("frequency")? {
                serde_json::Value::String(s) => s.parse().ok()?,
                serde_json::Value::Number(n) => n.as_f64()?,
                _ => return None,
            };
            let age_s = item
                .get("spotTime")
                .and_then(|v| v.as_str())
                .and_then(parse_iso_utc)
                .map(|ts| (now - ts).num_seconds().max(0))
                .unwrap_or(0);
            Some(Spot {
                callsign,
                freq_hz: (freq_khz * 1000.0).round() as u64,
                source: "pota".to_string(),
                snr_db: None,
                label: item
                    .get("reference")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                age_s: age_s as u32,
            })
        })
        .collect()
}

/// Merge, dedup by callsign (POTA wins over RBN; else freshest), age-filter, cap.
pub fn merge_spots(mut spots: Vec<Spot>) -> Vec<Spot> {
    spots.retain(|s| i64::from(s.age_s) <= MAX_AGE_SECS);
    let mut by_call: HashMap<String, Spot> = HashMap::new();
    for spot in spots {
        match by_call.get(&spot.callsign) {
            Some(existing) => {
                let replace = (spot.source == "pota" && existing.source != "pota")
                    || (spot.source == existing.source && spot.age_s < existing.age_s);
                if replace {
                    by_call.insert(spot.callsign.clone(), spot);
                }
            }
            None => {
                by_call.insert(spot.callsign.clone(), spot);
            }
        }
    }
    let mut merged: Vec<Spot> = by_call.into_values().collect();
    merged.sort_by_key(|s| s.age_s);
    merged.truncate(MAX_SPOTS);
    merged
}

async fn fetch_json(client: &reqwest::Client, url: &str) -> anyhow::Result<serde_json::Value> {
    Ok(client
        .get(url)
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

/// Poll RBN + POTA for the rig's current band and publish merged spots.
pub async fn run_spots_task(
    bands: Vec<BandConfig>,
    rig_rx: watch::Receiver<RigState>,
    spots_tx: watch::Sender<Vec<Spot>>,
    poll_secs: u64,
) {
    let client = match reqwest::Client::builder()
        .user_agent("k3fall/0.1 (+https://github.com/jsvana)")
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            tracing::warn!(?err, "spots: failed to build http client; spots disabled");
            return;
        }
    };
    let mut interval = tokio::time::interval(Duration::from_secs(poll_secs.max(10)));
    loop {
        interval.tick().await;
        let rig = rig_rx.borrow().clone();
        if !rig.connected || rig.vfo_a_hz == 0 {
            continue;
        }
        let Some(band) = band_name_for(&bands, rig.vfo_a_hz).map(str::to_string) else {
            continue;
        };
        let band_cfg = bands.iter().find(|b| b.name == band).cloned();
        let now = Utc::now();
        let mut all = Vec::new();

        let rbn_url = format!("https://vailrerbn.com/api/v1/spots?band={band}&limit=100");
        match fetch_json(&client, &rbn_url).await {
            Ok(body) => all.extend(parse_rbn(&body, now)),
            Err(err) => tracing::warn!(?err, "spots: rbn fetch failed"),
        }

        match fetch_json(&client, "https://api.pota.app/spot/activator").await {
            Ok(body) => {
                let mut pota = parse_pota(&body, now);
                if let Some(b) = &band_cfg {
                    pota.retain(|s| s.freq_hz >= b.low_hz && s.freq_hz <= b.high_hz);
                }
                all.extend(pota);
            }
            Err(err) => tracing::warn!(?err, "spots: pota fetch failed"),
        }

        let merged = merge_spots(all);
        tracing::debug!(count = merged.len(), band, "spots: updated");
        let _ = spots_tx.send(merged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 7, 23, 30, 0).unwrap()
    }

    #[test]
    fn parses_rbn_response() {
        let body: serde_json::Value = serde_json::from_str(
            r#"{"total":1,"spots":[{"callsign":"F6HKA","frequency":14008.1,"mode":"CW","timestamp":"2026-09-07T23:29:20Z","snr":22}]}"#,
        )
        .unwrap();
        let spots = parse_rbn(&body, now());
        assert_eq!(spots.len(), 1);
        assert_eq!(spots[0].callsign, "F6HKA");
        assert_eq!(spots[0].freq_hz, 14_008_100);
        assert_eq!(spots[0].snr_db, Some(22));
        assert_eq!(spots[0].age_s, 40);
    }

    #[test]
    fn parses_pota_response_with_string_frequency() {
        let body: serde_json::Value = serde_json::from_str(
            r#"[{"activator":"K4PDW","frequency":"14031","reference":"US-2169","mode":"CW","spotTime":"2026-09-07T23:25:00"}]"#,
        )
        .unwrap();
        let spots = parse_pota(&body, now());
        assert_eq!(spots.len(), 1);
        assert_eq!(spots[0].freq_hz, 14_031_000);
        assert_eq!(spots[0].label.as_deref(), Some("US-2169"));
        assert_eq!(spots[0].age_s, 300);
    }

    #[test]
    fn merge_dedups_prefers_pota_and_drops_stale() {
        let spots = vec![
            Spot {
                callsign: "K4PDW".into(),
                freq_hz: 14_031_050,
                source: "rbn".into(),
                snr_db: Some(15),
                label: None,
                age_s: 30,
            },
            Spot {
                callsign: "K4PDW".into(),
                freq_hz: 14_031_000,
                source: "pota".into(),
                snr_db: None,
                label: Some("US-2169".into()),
                age_s: 300,
            },
            Spot {
                callsign: "OLD1XX".into(),
                freq_hz: 14_020_000,
                source: "rbn".into(),
                snr_db: None,
                label: None,
                age_s: 999,
            },
        ];
        let merged = merge_spots(spots);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].source, "pota");
    }

    #[test]
    fn band_lookup_uses_config_names() {
        let bands = Config::default_dev().bands;
        assert_eq!(band_name_for(&bands, 14_032_500), Some("20m"));
        assert_eq!(band_name_for(&bands, 5_000_000), None);
    }
}
