#![allow(dead_code)]

use anyhow::Result;
use num_complex::Complex;
use std::f32::consts::TAU;
use std::path::Path;

pub trait IqSource: Send {
    fn read(&mut self, buf: &mut [Complex<f32>]) -> Result<usize>;
    fn sample_rate(&self) -> u32;
}

pub struct FileSource {
    samples: Vec<Complex<f32>>,
    pos: usize,
    sample_rate: u32,
}

impl FileSource {
    pub fn from_samples(samples: Vec<Complex<f32>>, sample_rate: u32) -> Self {
        assert!(!samples.is_empty());
        Self {
            samples,
            pos: 0,
            sample_rate,
        }
    }

    pub fn open(path: &Path, sample_rate: u32) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        let samples = bytes
            .chunks_exact(8)
            .map(|c| {
                let re = f32::from_le_bytes(c[0..4].try_into().unwrap());
                let im = f32::from_le_bytes(c[4..8].try_into().unwrap());
                Complex::new(re, im)
            })
            .collect::<Vec<_>>();
        anyhow::ensure!(!samples.is_empty(), "empty IQ file");
        Ok(Self::from_samples(samples, sample_rate))
    }
}

impl IqSource for FileSource {
    fn read(&mut self, buf: &mut [Complex<f32>]) -> Result<usize> {
        for out in buf.iter_mut() {
            *out = self.samples[self.pos];
            self.pos = (self.pos + 1) % self.samples.len();
        }
        Ok(buf.len())
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

pub fn tone(sample_rate: u32, freq_hz: f32, amplitude: f32, len: usize) -> Vec<Complex<f32>> {
    (0..len)
        .map(|n| {
            let ph = TAU * freq_hz * n as f32 / sample_rate as f32;
            Complex::new(amplitude * ph.cos(), amplitude * ph.sin())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_source_loops() {
        let samples = tone(250_000, 1_000.0, 1.0, 4096);
        let mut src = FileSource::from_samples(samples.clone(), 250_000);
        let mut buf = vec![Complex::new(0.0f32, 0.0); 6000]; // > len, forces wrap
        let n = src.read(&mut buf).unwrap();
        assert_eq!(n, 6000);
        assert_eq!(buf[4096], samples[0]); // wrapped to start
    }

    #[test]
    fn tone_has_unit_magnitude() {
        let t = tone(250_000, -10_000.0, 1.0, 100);
        assert!((t[50].norm() - 1.0).abs() < 1e-5);
    }
}
