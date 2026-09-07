use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub spots: SpotsConfig,
    pub sdr: SdrConfig,
    pub cat: CatConfig,
    pub dsp: DspConfig,
    pub server: ServerConfig,
    pub bands: Vec<BandConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdrConfig {
    pub driver: String,
    pub file_path: Option<PathBuf>,
    pub center_hz: u64,
    pub sample_rate: u32,
    pub gain_db: f32,
    pub antenna: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatConfig {
    pub enabled: bool,
    pub device: String,
    pub baud: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DspConfig {
    pub fft_size: usize,
    pub rows_per_sec: u32,
    pub ref_level_db: f32,
    pub range_db: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub listen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandConfig {
    pub name: String,
    pub low_hz: u64,
    pub high_hz: u64,
    pub direction: i8,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let config = toml::from_str(&content)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
        Ok(config)
    }

    pub fn default_dev() -> Config {
        Config {
            spots: SpotsConfig::default(),
            sdr: SdrConfig {
                driver: "file".to_string(),
                file_path: None,
                center_hz: 8_215_000,
                sample_rate: 250_000,
                gain_db: 0.0,
                antenna: None,
            },
            cat: CatConfig {
                enabled: false,
                device: String::new(),
                baud: 9600,
            },
            dsp: DspConfig {
                fft_size: 32768,
                rows_per_sec: 30,
                ref_level_db: -30.0,
                range_db: 90.0,
            },
            server: ServerConfig {
                listen: "127.0.0.1:8080".to_string(),
            },
            bands: default_bands(),
        }
    }
}

fn default_bands() -> Vec<BandConfig> {
    vec![
        BandConfig {
            name: "160m".to_string(),
            low_hz: 1_800_000,
            high_hz: 2_000_000,
            direction: -1,
        },
        BandConfig {
            name: "80m".to_string(),
            low_hz: 3_500_000,
            high_hz: 4_000_000,
            direction: -1,
        },
        BandConfig {
            name: "40m".to_string(),
            low_hz: 7_000_000,
            high_hz: 7_300_000,
            direction: -1,
        },
        BandConfig {
            name: "30m".to_string(),
            low_hz: 10_100_000,
            high_hz: 10_150_000,
            direction: 1,
        },
        BandConfig {
            name: "20m".to_string(),
            low_hz: 14_000_000,
            high_hz: 14_350_000,
            direction: 1,
        },
        BandConfig {
            name: "17m".to_string(),
            low_hz: 18_068_000,
            high_hz: 18_168_000,
            direction: 1,
        },
        BandConfig {
            name: "15m".to_string(),
            low_hz: 21_000_000,
            high_hz: 21_450_000,
            direction: 1,
        },
        BandConfig {
            name: "12m".to_string(),
            low_hz: 24_890_000,
            high_hz: 24_990_000,
            direction: 1,
        },
        BandConfig {
            name: "10m".to_string(),
            low_hz: 28_000_000,
            high_hz: 29_700_000,
            direction: 1,
        },
        BandConfig {
            name: "6m".to_string(),
            low_hz: 50_000_000,
            high_hz: 54_000_000,
            direction: 1,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example_config() {
        let cfg = Config::load(std::path::Path::new("config.example.toml")).unwrap();
        assert_eq!(cfg.sdr.center_hz, 8_215_000);
        assert_eq!(cfg.dsp.fft_size, 32768);
        assert_eq!(
            cfg.bands
                .iter()
                .find(|b| b.name == "20m")
                .unwrap()
                .direction,
            1
        );
    }

    #[test]
    fn default_dev_uses_file_driver() {
        let cfg = Config::default_dev();
        assert_eq!(cfg.sdr.driver, "file");
        assert!(!cfg.cat.enabled);
    }
}

/// Spot enrichment sources (RBN + POTA). Defaults on; absent from older
/// config files, hence the serde defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpotsConfig {
    #[serde(default = "default_spots_enabled")]
    pub enabled: bool,
    #[serde(default = "default_spots_poll_secs")]
    pub poll_secs: u64,
}

impl Default for SpotsConfig {
    fn default() -> Self {
        Self {
            enabled: default_spots_enabled(),
            poll_secs: default_spots_poll_secs(),
        }
    }
}

fn default_spots_enabled() -> bool {
    true
}

fn default_spots_poll_secs() -> u64 {
    30
}
