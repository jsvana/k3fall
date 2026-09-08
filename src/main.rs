use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use k3fall::{cat, config::Config, dsp, iq, remoteop, spots, web};

/// Called by the DSP thread's reconnect loop to (re)acquire an `IqSource`.
/// Invoked once at startup and again after every stream/read failure, so it
/// must fully rebuild the source rather than reuse any prior hardware state.
type SourceFactory = Box<dyn FnMut() -> anyhow::Result<Box<dyn iq::IqSource>> + Send>;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    let config_path = args.get(1).map(String::as_str).unwrap_or("config.toml");
    let path = Path::new(config_path);

    let (cfg, make_source, pace) = if path.exists() {
        match Config::load(path) {
            Ok(cfg) => {
                // Only the synthetic `file` driver needs the DSP thread to
                // pace itself with `sleep`s; real hardware sources block in
                // their own `read()` until samples are actually available,
                // so pacing on top of that would starve them and cause
                // permanent overrun (see finding C1).
                let pace = cfg.sdr.driver == "file";
                let make_source = build_source(&cfg);
                (cfg, make_source, pace)
            }
            Err(err) => {
                // The config file exists but couldn't be read/parsed: this
                // is almost certainly an operator mistake (bad TOML,
                // permissions), and silently falling back to dev mode would
                // hide it under systemd (no listen port on LAN, no real
                // signal). Fail fast instead so the error is visible.
                tracing::error!("failed to load config {}: {err:#}", path.display());
                std::process::exit(1);
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
    let (sdr_ok_tx, sdr_ok_rx) = tokio::sync::watch::channel(false);

    let (rows_tx, _rows_rx) = tokio::sync::broadcast::channel::<dsp::SpectrumRow>(32);

    {
        let rows_tx = rows_tx.clone();
        let dsp_cfg = cfg.dsp.clone();
        std::thread::spawn(move || {
            dsp::run_dsp_thread(make_source, dsp_cfg, rows_tx, pace, sdr_ok_tx)
        });
    }

    let (rig_tx, rig_rx) = tokio::sync::watch::channel(cat::RigState::default());
    let (tune_tx, tune_rx) = tokio::sync::mpsc::channel::<u64>(32);
    let (cat_raw_tx, cat_raw_rx) = tokio::sync::mpsc::channel::<String>(64);

    if cfg.cat.enabled {
        let cat_cfg = cfg.cat.clone();
        tokio::spawn(cat::run_cat_task(cat_cfg, rig_tx, tune_rx, cat_raw_rx));
    } else {
        // Keep the watch sender alive for the life of the program so
        // `rig_rx`'s `changed()` never observes a closed channel; nothing
        // else holds `rig_tx` when CAT is disabled.
        tokio::spawn(async move {
            let _keep_alive = rig_tx;
            std::future::pending::<()>().await;
        });
    }

    let (spots_tx, spots_rx) = tokio::sync::watch::channel(Vec::new());
    if cfg.spots.enabled {
        tokio::spawn(spots::run_spots_task(
            cfg.bands.clone(),
            rig_rx.clone(),
            spots_tx,
            cfg.spots.poll_secs,
        ));
    } else {
        tokio::spawn(async move {
            let _keep_alive = spots_tx;
            std::future::pending::<()>().await;
        });
    }

    let (audio_tx, _audio_rx) = tokio::sync::broadcast::channel::<k3fall::audio::AudioPacket>(64);
    if cfg.audio.enabled {
        k3fall::audio::spawn_capture(cfg.audio.clone(), audio_tx.clone());
    }

    let remote_state = remoteop::RemoteState::load(cfg.remoteop.state_file.clone());
    if cfg.remoteop.enabled {
        let deps = std::sync::Arc::new(remoteop::Deps {
            cfg: cfg.clone(),
            rows: rows_tx.clone(),
            rig: rig_rx.clone(),
            spots: spots_rx.clone(),
            tune: tune_tx.clone(),
            cat_raw: cat_raw_tx.clone(),
            state: remote_state.clone(),
            audio: audio_tx.clone(),
            audio_enabled: cfg.audio.enabled,
        });
        tokio::spawn(remoteop::run_server(cfg.remoteop.listen.clone(), deps));
    }

    let state = web::AppState {
        rows: rows_tx,
        rig: rig_rx,
        tune: tune_tx,
        cfg: cfg.clone(),
        sdr_ok_rx,
        spots_rx,
        remote: remote_state,
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

/// Build a `SourceFactory` from a loaded config.
///
/// The `file` driver re-opens the file from disk on every call, which is
/// what lets the DSP thread's reconnect loop recover if the file
/// disappears/reappears. The `sdrplay` driver requires the `soapy` cargo
/// feature; without it we fail fast with a clear error rather than looping
/// forever on an unsupported driver.
fn build_source(cfg: &Config) -> SourceFactory {
    match cfg.sdr.driver.as_str() {
        "file" => {
            let path = cfg
                .sdr
                .file_path
                .clone()
                .unwrap_or_else(|| panic!("sdr.driver = \"file\" requires sdr.file_path"));
            let sample_rate = cfg.sdr.sample_rate;
            Box::new(move || {
                let source = iq::FileSource::open(&path, sample_rate)
                    .with_context(|| format!("failed to open IQ file {}", path.display()))?;
                Ok(Box::new(source) as Box<dyn iq::IqSource>)
            })
        }
        "sdrplay" => {
            #[cfg(feature = "soapy")]
            {
                let sdr_cfg = cfg.sdr.clone();
                Box::new(move || {
                    let source = k3fall::soapy::SoapySource::open(&sdr_cfg)?;
                    Ok(Box::new(source) as Box<dyn iq::IqSource>)
                })
            }
            #[cfg(not(feature = "soapy"))]
            {
                panic!(
                    "sdr.driver = \"sdrplay\" requires building with `--features soapy` \
                     (this build was compiled without SoapySDR support)"
                )
            }
        }
        other => {
            panic!("unsupported sdr driver '{other}': only 'file' and 'sdrplay' are implemented")
        }
    }
}

/// Dev-mode fallback used when no config file is found: a synthesized
/// multi-tone signal standing in for a real SDR/K3 IF feed.
fn dev_mode() -> (Config, SourceFactory, bool) {
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

    // Cache the synthesized signal and hand back a fresh clone each time the
    // factory is called, so the DSP thread's reconnect loop behaves the same
    // way here as it does for real sources.
    let signal = Arc::new(signal);
    let make_source: SourceFactory = Box::new(move || {
        Ok(
            Box::new(iq::FileSource::from_samples((*signal).clone(), sample_rate))
                as Box<dyn iq::IqSource>,
        )
    });
    (cfg, make_source, true)
}
