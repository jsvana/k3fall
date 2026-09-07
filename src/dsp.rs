use crate::config::DspConfig;
use crate::iq::IqSource;
use num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::sync::Arc;

#[derive(Clone)]
pub struct SpectrumRow {
    pub seq: u32,
    pub bins: Arc<Vec<u8>>,
}

pub struct Dsp {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    fft_size: usize,
    hop: usize,
    sample_rate: u32,
    ref_level_db: f32,
    range_db: f32,
    buffer: VecDeque<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

fn compute_hop(sample_rate: u32, rows_per_sec: u32) -> usize {
    ((sample_rate / rows_per_sec.max(1)) as usize).max(1)
}

impl Dsp {
    pub fn new(cfg: &DspConfig, sample_rate: u32) -> Self {
        let fft_size = cfg.fft_size;
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(fft_size);
        let window: Vec<f32> = (0..fft_size)
            .map(|n| 0.5 * (1.0 - (TAU * n as f32 / (fft_size - 1) as f32).cos()))
            .collect();
        let hop = compute_hop(sample_rate, cfg.rows_per_sec);
        Self {
            fft,
            window,
            fft_size,
            hop,
            sample_rate,
            ref_level_db: cfg.ref_level_db,
            range_db: cfg.range_db,
            buffer: VecDeque::with_capacity(fft_size * 2),
            scratch: vec![Complex::new(0.0, 0.0); fft_size],
        }
    }

    pub fn bin_hz(&self) -> f32 {
        self.sample_rate as f32 / self.fft_size as f32
    }

    pub fn process(&mut self, samples: &[Complex<f32>]) -> Vec<Vec<u8>> {
        self.buffer.extend(samples.iter().copied());
        let mut rows = Vec::new();
        while self.buffer.len() >= self.fft_size {
            for (i, s) in self.buffer.iter().take(self.fft_size).enumerate() {
                self.scratch[i] = *s * self.window[i];
            }
            self.fft.process(&mut self.scratch);

            let n = self.fft_size;
            let n_sqr = n as f32 * n as f32;
            let db: Vec<f32> = self
                .scratch
                .iter()
                .map(|c| 10.0 * (c.norm_sqr() / n_sqr + 1e-20).log10())
                .collect();

            // fft-shift: second half of FFT output first, then first half,
            // so shifted index 0 = -fs/2 and shifted index n/2 = DC.
            let half = n / 2;
            let mut shifted = vec![0.0f32; n];
            shifted[..n - half].copy_from_slice(&db[half..]);
            shifted[n - half..].copy_from_slice(&db[..half]);

            rows.push(quantize(&shifted, self.ref_level_db, self.range_db));

            let drain_n = self.hop.min(self.buffer.len());
            self.buffer.drain(..drain_n);
        }
        rows
    }
}

pub fn quantize(db: &[f32], ref_level_db: f32, range_db: f32) -> Vec<u8> {
    let floor = ref_level_db - range_db;
    db.iter()
        .map(|&v| {
            let frac = (v - floor) / range_db;
            (frac * 255.0).clamp(0.0, 255.0).round() as u8
        })
        .collect()
}

pub fn run_dsp_thread(
    source: Box<dyn IqSource>,
    cfg: DspConfig,
    tx: tokio::sync::broadcast::Sender<SpectrumRow>,
    pace: bool,
) {
    let mut source = source;
    let sample_rate = source.sample_rate();
    let mut dsp = Dsp::new(&cfg, sample_rate);
    let hop = compute_hop(sample_rate, cfg.rows_per_sec);
    let mut buf = vec![Complex::new(0.0f32, 0.0); hop];
    let sleep_dur = std::time::Duration::from_secs_f64(hop as f64 / sample_rate as f64);
    let mut seq: u32 = 0;

    loop {
        match source.read(&mut buf) {
            Ok(n) => {
                let rows = dsp.process(&buf[..n]);
                for bins in rows {
                    let row = SpectrumRow {
                        seq,
                        bins: Arc::new(bins),
                    };
                    seq = seq.wrapping_add(1);
                    let _ = tx.send(row);
                }
                if pace {
                    std::thread::sleep(sleep_dur);
                }
            }
            Err(e) => {
                tracing::warn!("dsp source read error: {e}");
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DspConfig;
    use crate::iq::tone;

    fn cfg() -> DspConfig {
        DspConfig {
            fft_size: 4096,
            rows_per_sec: 30,
            ref_level_db: 0.0,
            range_db: 90.0,
        }
    }

    #[test]
    fn tone_lands_in_expected_bin() {
        let sample_rate = 250_000u32;
        let mut dsp = Dsp::new(&cfg(), sample_rate);
        let samples = tone(sample_rate, 10_000.0, 0.5, 5 * 4096);
        let rows = dsp.process(&samples);
        assert!(!rows.is_empty());
        let row = &rows[0];
        let bin_hz = sample_rate as f32 / 4096.0;
        let expected = (4096 / 2) as f32 + 10_000.0 / bin_hz; // center + offset
        let peak = row.iter().enumerate().max_by_key(|(_, v)| **v).unwrap().0;
        assert!(
            (peak as f32 - expected).abs() <= 1.0,
            "peak {peak} expected {expected}"
        );
    }

    #[test]
    fn negative_freq_lands_below_center() {
        let sample_rate = 250_000u32;
        let mut dsp = Dsp::new(&cfg(), sample_rate);
        let rows = dsp.process(&tone(sample_rate, -50_000.0, 0.5, 5 * 4096));
        let peak = rows[0]
            .iter()
            .enumerate()
            .max_by_key(|(_, v)| **v)
            .unwrap()
            .0;
        assert!(peak < 4096 / 2);
    }

    #[test]
    fn quantize_maps_ref_to_255_and_floor_to_0() {
        let q = quantize(&[0.0, -90.0, -45.0, 10.0, -200.0], 0.0, 90.0);
        assert_eq!(q[0], 255);
        assert_eq!(q[1], 0);
        assert!((q[2] as i32 - 127).abs() <= 1);
        assert_eq!(q[3], 255); // clamped high
        assert_eq!(q[4], 0); // clamped low
    }
}
