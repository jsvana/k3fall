//! SoapySDR-backed [`IqSource`] for the SDRplay RSP2pro.
//!
//! This module is only compiled with `--features soapy`, which links against
//! the native SoapySDR library (and its SDRplay module) via the `soapysdr`
//! crate. It has no coverage in the default build/test suite: the Pi is the
//! only environment with the native library and the actual hardware, so
//! verification of this module is a build + hardware smoke test on the Pi,
//! not `cargo test` here.

use crate::config::SdrConfig;
use crate::iq::IqSource;
use anyhow::{Context, Result};
use num_complex::Complex;
use soapysdr::{Device, Direction, RxStream};

/// The RSP2pro's single RX channel.
const CHANNEL: usize = 0;

/// IF bandwidth we ask SoapySDR to configure. Not every SoapySDR module
/// honors this (or has a matching filter option); a failure to set it is
/// non-fatal and only logged.
const IF_BANDWIDTH_HZ: f64 = 200_000.0;

/// Timeout for a single stream read, in microseconds.
const READ_TIMEOUT_US: i64 = 1_000_000;

/// An `IqSource` backed by a real SDRplay RSP2pro via SoapySDR.
///
/// The stream keeps its own handle to the underlying `Device` (the
/// `soapysdr` crate's `Device` is a cheap `Arc`-backed clone), so there is no
/// need to separately hold the `Device` here.
pub struct SoapySource {
    stream: RxStream<Complex<f32>>,
    sample_rate: u32,
}

impl SoapySource {
    /// Open the SDRplay RSP2pro and configure it per `cfg`.
    ///
    /// Passing only `driver=sdrplay` (and nothing more specific) lets the
    /// SoapySDRPlay module pick the attached RSP2pro; this box only ever has
    /// one RSPplay-family device attached.
    pub fn open(cfg: &SdrConfig) -> Result<Self> {
        let device =
            Device::new("driver=sdrplay").context("failed to open SoapySDR sdrplay device")?;

        device
            .set_sample_rate(Direction::Rx, CHANNEL, cfg.sample_rate as f64)
            .context("failed to set sample rate")?;

        device
            .set_frequency(Direction::Rx, CHANNEL, cfg.center_hz as f64, ())
            .context("failed to set center frequency")?;

        device
            .set_gain(Direction::Rx, CHANNEL, cfg.gain_db as f64)
            .context("failed to set gain")?;

        if let Some(antenna) = cfg.antenna.as_deref() {
            device
                .set_antenna(Direction::Rx, CHANNEL, antenna)
                .with_context(|| format!("failed to set antenna {antenna:?}"))?;
        }

        if let Err(err) = device.set_bandwidth(Direction::Rx, CHANNEL, IF_BANDWIDTH_HZ) {
            tracing::warn!(
                "sdrplay: driver did not accept IF bandwidth {IF_BANDWIDTH_HZ} Hz: {err}"
            );
        }

        let mut stream = device
            .rx_stream::<Complex<f32>>(&[CHANNEL])
            .context("failed to set up RX stream")?;

        stream
            .activate(None)
            .context("failed to activate RX stream")?;

        Ok(Self {
            stream,
            sample_rate: cfg.sample_rate,
        })
    }
}

impl IqSource for SoapySource {
    fn read(&mut self, buf: &mut [Complex<f32>]) -> Result<usize> {
        self.stream
            .read(&mut [buf], READ_TIMEOUT_US)
            .map_err(|err| anyhow::anyhow!("sdrplay stream read error: {err}"))
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}
