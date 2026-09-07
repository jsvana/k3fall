use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use k3fall::{cat, config::Config, dsp, iq, web};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    let config_path = args.get(1).map(String::as_str).unwrap_or("config.toml");
    let path = Path::new(config_path);

    let (cfg, source, pace) = if path.exists() {
        match Config::load(path) {
            Ok(cfg) => {
                let source = build_source(&cfg);
                (cfg, source, true)
            }
            Err(err) => {
                tracing::error!(
                    "failed to load config {}: {err}; falling back to dev mode",
                    path.display()
                );
                dev_mode()
            }
        }
    } else {
        tracing::info!(
            "no config file at {}; using dev mode (synthesized tones)",
            path.display()
        );
        dev_mode()
    };

    let cfg = Arc::new(cfg);
    let sdr_ok = Arc::new(AtomicBool::new(false));

    let (rows_tx, _rows_rx) = tokio::sync::broadcast::channel::<dsp::SpectrumRow>(32);

    {
        let rows_tx = rows_tx.clone();
        let dsp_cfg = cfg.dsp.clone();
        // A `file`-backed source is considered working the moment it's been
        // constructed, so mark the SDR healthy right before handing it off
        // to the DSP thread. Task 9 makes this reflect real hardware state.
        sdr_ok.store(true, Ordering::Relaxed);
        std::thread::spawn(move || dsp::run_dsp_thread(source, dsp_cfg, rows_tx, pace));
    }

    let (rig_tx, rig_rx) = tokio::sync::watch::channel(cat::RigState::default());
    let (tune_tx, tune_rx) = tokio::sync::mpsc::channel::<u64>(32);

    if cfg.cat.enabled {
        let cat_cfg = cfg.cat.clone();
        tokio::spawn(cat::run_cat_task(cat_cfg, rig_tx, tune_rx));
    } else {
        // Keep the watch sender alive for the life of the program so
        // `rig_rx`'s `changed()` never observes a closed channel; nothing
        // else holds `rig_tx` when CAT is disabled.
        tokio::spawn(async move {
            let _keep_alive = rig_tx;
            std::future::pending::<()>().await;
        });
    }

    let state = web::AppState {
        rows: rows_tx,
        rig: rig_rx,
        tune: tune_tx,
        cfg: cfg.clone(),
        sdr_ok,
    };

    let app = web::router(state);
    let listener = tokio::net::TcpListener::bind(&cfg.server.listen)
        .await
        .unwrap_or_else(|err| panic!("failed to bind {}: {err}", cfg.server.listen));
    tracing::info!("k3fall listening on {}", cfg.server.listen);
    axum::serve(listener, app)
        .await
        .expect("axum server exited with an error");
}

/// Build an `IqSource` from a loaded config. Only the `file` driver is
/// implemented in this build; hardware drivers (e.g. SoapySDR) are out of
/// scope for the web server task.
fn build_source(cfg: &Config) -> Box<dyn iq::IqSource> {
    match cfg.sdr.driver.as_str() {
        "file" => {
            let path = cfg
                .sdr
                .file_path
                .as_ref()
                .unwrap_or_else(|| panic!("sdr.driver = \"file\" requires sdr.file_path"));
            let source = iq::FileSource::open(path, cfg.sdr.sample_rate)
                .unwrap_or_else(|err| panic!("failed to open IQ file {}: {err}", path.display()));
            Box::new(source)
        }
        other => {
            panic!("unsupported sdr driver '{other}': only 'file' is implemented in this build")
        }
    }
}

/// Dev-mode fallback used when no config file is found: a synthesized
/// multi-tone signal standing in for a real SDR/K3 IF feed.
fn dev_mode() -> (Config, Box<dyn iq::IqSource>, bool) {
    let cfg = Config::default_dev();
    let sample_rate = cfg.sdr.sample_rate;
    let len = sample_rate as usize * 2; // 2 seconds of samples

    let mut signal = iq::tone(sample_rate, 0.0, 0.15, len);
    for (freq_hz, amplitude) in [
        (-80_000.0, 0.02),
        (-30_000.0, 0.10),
        (12_000.0, 0.05),
        (41_000.0, 0.03),
    ] {
        let tone = iq::tone(sample_rate, freq_hz, amplitude, len);
        for (sample, t) in signal.iter_mut().zip(tone.iter()) {
            *sample += *t;
        }
    }

    let source: Box<dyn iq::IqSource> = Box::new(iq::FileSource::from_samples(signal, sample_rate));
    (cfg, source, true)
}
