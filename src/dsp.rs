use crate::config::DspConfig;
use crate::iq::IqSource;
use num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// How long to wait between (re)connect attempts after a source fails to
/// open or a stream read errors out.
const RECONNECT_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

/// Drive the DSP pipeline forever, (re)acquiring the IQ source as needed.
///
/// `make_source` is called each time a source is needed: once at startup,
/// and again any time the current source fails a read (e.g. the SDR was
/// unplugged). This lets hardware sources be re-opened from scratch on
/// reconnect while file/dev sources can simply hand back a fresh copy.
///
/// `sdr_ok` reflects whether a source is currently open and being read from
/// successfully; it flips to `false` while there is no live source (either
/// because `make_source` is failing, or the previous source's stream just
/// errored and hasn't been replaced yet).
pub fn run_dsp_thread(
    make_source: Box<dyn FnMut() -> anyhow::Result<Box<dyn IqSource>> + Send>,
    cfg: DspConfig,
    tx: tokio::sync::broadcast::Sender<SpectrumRow>,
    pace: bool,
    sdr_ok: Arc<AtomicBool>,
) {
    run_dsp_thread_with_delay(make_source, cfg, tx, pace, sdr_ok, RECONNECT_DELAY)
}

/// Real implementation behind [`run_dsp_thread`], with the reconnect delay
/// as a parameter so tests can drive the retry loop in milliseconds instead
/// of waiting on the real 3s production delay.
pub fn run_dsp_thread_with_delay(
    mut make_source: Box<dyn FnMut() -> anyhow::Result<Box<dyn IqSource>> + Send>,
    cfg: DspConfig,
    tx: tokio::sync::broadcast::Sender<SpectrumRow>,
    pace: bool,
    sdr_ok: Arc<AtomicBool>,
    reconnect_delay: std::time::Duration,
) {
    let mut seq: u32 = 0;

    loop {
        let mut source = match make_source() {
            Ok(source) => source,
            Err(err) => {
                sdr_ok.store(false, Ordering::Relaxed);
                tracing::warn!("failed to open IQ source: {err}; retrying in {reconnect_delay:?}");
                std::thread::sleep(reconnect_delay);
                continue;
            }
        };
        sdr_ok.store(true, Ordering::Relaxed);

        let sample_rate = source.sample_rate();
        let mut dsp = Dsp::new(&cfg, sample_rate);
        let hop = compute_hop(sample_rate, cfg.rows_per_sec);
        let mut buf = vec![Complex::new(0.0f32, 0.0); hop];
        let sleep_dur = std::time::Duration::from_secs_f64(hop as f64 / sample_rate as f64);

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
                    sdr_ok.store(false, Ordering::Relaxed);
                    tracing::warn!(
                        "dsp source read error: {e}; dropping source and retrying in {reconnect_delay:?}"
                    );
                    break;
                }
            }
        }
        // `source` is dropped here, releasing any hardware handle, before
        // the outer loop retries `make_source`.
        drop(source);
        std::thread::sleep(reconnect_delay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DspConfig;
    use crate::iq::{tone, FileSource};
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    fn cfg() -> DspConfig {
        DspConfig {
            fft_size: 4096,
            rows_per_sec: 30,
            ref_level_db: 0.0,
            range_db: 90.0,
        }
    }

    /// A tiny config/sample-rate pairing chosen so that every successful
    /// `read()` (which pulls exactly `hop` samples) fills the FFT buffer
    /// and emits exactly one row: `hop == compute_hop(64, 1) == 64 ==
    /// fft_size`. This keeps the reconnect tests below fast and
    /// deterministic without needing to wait on real timing.
    fn reconnect_test_cfg() -> DspConfig {
        DspConfig {
            fft_size: 64,
            rows_per_sec: 1,
            ref_level_db: 0.0,
            range_db: 90.0,
        }
    }

    const RECONNECT_TEST_SAMPLE_RATE: u32 = 64;

    fn reconnect_test_tone() -> Vec<Complex<f32>> {
        tone(RECONNECT_TEST_SAMPLE_RATE, 8.0, 0.5, 256)
    }

    /// An `IqSource` that answers `reads_before_fail` reads from an inner
    /// looping tone source, then fails every read after that. Used to
    /// simulate a hardware source whose stream errors out mid-flight (e.g.
    /// the SDR gets unplugged).
    struct FlakySource {
        inner: FileSource,
        reads_before_fail: usize,
        read_count: usize,
    }

    impl IqSource for FlakySource {
        fn read(&mut self, buf: &mut [Complex<f32>]) -> anyhow::Result<usize> {
            if self.read_count >= self.reads_before_fail {
                anyhow::bail!("synthetic read failure");
            }
            self.read_count += 1;
            self.inner.read(buf)
        }

        fn sample_rate(&self) -> u32 {
            self.inner.sample_rate()
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

    // The two tests below drive `run_dsp_thread_with_delay` on a real
    // `std::thread`, same as production, since the reconnect loop is
    // intentionally blocking (`std::thread::sleep`) rather than async.
    // Nothing in the loop ever returns, so there is no clean join handle to
    // wait on; rather than invent a shutdown signal that production doesn't
    // have (and that the finding explicitly warns off — dropping receivers
    // does not stop the sender loop, since `tx.send` errors are ignored),
    // each test spawns its thread detached and lets it run for the rest of
    // the test binary's lifetime. That's safe here: once `main` (in this
    // case the test harness) returns, the process exits and the OS reclaims
    // the thread along with everything else — nothing to join, nothing left
    // running afterward.

    #[tokio::test]
    async fn reconnect_retries_then_succeeds_and_streams_rows() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let samples = reconnect_test_tone();

        let make_source: Box<dyn FnMut() -> anyhow::Result<Box<dyn IqSource>> + Send> = {
            let call_count = call_count.clone();
            Box::new(move || {
                let attempt = call_count.fetch_add(1, Ordering::SeqCst) + 1;
                if attempt <= 2 {
                    anyhow::bail!("synthetic open failure #{attempt}");
                }
                let src: Box<dyn IqSource> = Box::new(FileSource::from_samples(
                    samples.clone(),
                    RECONNECT_TEST_SAMPLE_RATE,
                ));
                Ok(src)
            })
        };

        let (tx, mut rx) = tokio::sync::broadcast::channel::<SpectrumRow>(16);
        let sdr_ok = Arc::new(AtomicBool::new(false));

        {
            let sdr_ok = sdr_ok.clone();
            std::thread::spawn(move || {
                run_dsp_thread_with_delay(
                    make_source,
                    reconnect_test_cfg(),
                    tx,
                    false,
                    sdr_ok,
                    Duration::from_millis(5),
                );
            });
        }

        let row = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for the first row")
            .expect("broadcast channel closed unexpectedly");
        assert!(!row.bins.is_empty());
        assert!(
            sdr_ok.load(Ordering::SeqCst),
            "sdr_ok should be true once a source is open and streaming"
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            3,
            "make_source should be called twice more after the first two failures"
        );
    }

    #[tokio::test]
    async fn read_error_flips_sdr_ok_and_resumes_from_replacement_source() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let samples = reconnect_test_tone();

        let make_source: Box<dyn FnMut() -> anyhow::Result<Box<dyn IqSource>> + Send> = {
            let call_count = call_count.clone();
            let samples = samples.clone();
            Box::new(move || {
                let attempt = call_count.fetch_add(1, Ordering::SeqCst) + 1;
                let src: Box<dyn IqSource> = if attempt == 1 {
                    // Produces exactly one row, then errors on every read
                    // after that -- simulating a source whose stream dies
                    // mid-flight.
                    Box::new(FlakySource {
                        inner: FileSource::from_samples(
                            samples.clone(),
                            RECONNECT_TEST_SAMPLE_RATE,
                        ),
                        reads_before_fail: 1,
                        read_count: 0,
                    })
                } else {
                    Box::new(FileSource::from_samples(
                        samples.clone(),
                        RECONNECT_TEST_SAMPLE_RATE,
                    ))
                };
                Ok(src)
            })
        };

        let (tx, mut rx) = tokio::sync::broadcast::channel::<SpectrumRow>(16);
        let sdr_ok = Arc::new(AtomicBool::new(false));

        {
            let sdr_ok = sdr_ok.clone();
            std::thread::spawn(move || {
                run_dsp_thread_with_delay(
                    make_source,
                    reconnect_test_cfg(),
                    tx,
                    false,
                    sdr_ok,
                    Duration::from_millis(5),
                );
            });
        }

        // The flaky source produces exactly one row before its next read
        // errors out.
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for the row from the flaky source")
            .expect("broadcast channel closed unexpectedly");

        // sdr_ok should flip false once that next read errors and the
        // source is dropped, ahead of the replacement being opened.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while sdr_ok.load(Ordering::SeqCst) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "sdr_ok never flipped false after the read error"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }

        // Rows should resume from the replacement source, and sdr_ok should
        // flip back to true once it's open.
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for a row from the replacement source")
            .expect("broadcast channel closed unexpectedly");
        assert!(
            sdr_ok.load(Ordering::SeqCst),
            "sdr_ok should be true again once the replacement source is streaming"
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "make_source should have been called once for the flaky source and once for its replacement"
        );
    }
}
