//! Rig audio capture: ALSA capture from a USB sound device on the K3's
//! line out, broadcast as 20 ms packets of S16LE mono PCM for the RemoteOp
//! `radio.audio` topic. Linux-only (the Pi); a stub elsewhere so macOS
//! development builds keep working.

use tokio::sync::broadcast;

/// One 20 ms packet of S16LE mono PCM at the configured sample rate.
#[derive(Debug, Clone)]
pub struct AudioPacket {
    pub seq: u32,
    pub bytes: Vec<u8>,
}

#[cfg(target_os = "linux")]
pub fn spawn_capture(cfg: crate::config::AudioConfig, tx: broadcast::Sender<AudioPacket>) {
    std::thread::spawn(move || {
        let packet_frames = (cfg.sample_rate / 50) as usize; // 20 ms
        let mut seq: u32 = 0;
        loop {
            match open_pcm(&cfg) {
                Ok(pcm) => {
                    tracing::info!(device = %cfg.device, rate = cfg.sample_rate, "audio: capturing");
                    let io = match pcm.io_i16() {
                        Ok(io) => io,
                        Err(err) => {
                            tracing::warn!(?err, "audio: io handle failed");
                            std::thread::sleep(std::time::Duration::from_secs(3));
                            continue;
                        }
                    };
                    let mut buf = vec![0i16; packet_frames];
                    loop {
                        match io.readi(&mut buf) {
                            Ok(read_frames) if read_frames > 0 => {
                                let bytes: Vec<u8> = buf[..read_frames]
                                    .iter()
                                    .flat_map(|s| s.to_le_bytes())
                                    .collect();
                                seq = seq.wrapping_add(1);
                                let _ = tx.send(AudioPacket { seq, bytes });
                            }
                            Ok(_) => {}
                            Err(err) => {
                                if pcm.try_recover(err, true).is_err() {
                                    tracing::warn!(?err, "audio: capture error; reopening");
                                    break;
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    tracing::debug!(?err, device = %cfg.device, "audio: open failed; retrying");
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    });
}

#[cfg(target_os = "linux")]
fn open_pcm(cfg: &crate::config::AudioConfig) -> anyhow::Result<alsa::pcm::PCM> {
    use alsa::pcm::{Access, Format, HwParams, PCM};
    let pcm = PCM::new(&cfg.device, alsa::Direction::Capture, false)?;
    {
        let hwp = HwParams::any(&pcm)?;
        hwp.set_channels(1)?;
        hwp.set_rate(cfg.sample_rate, alsa::ValueOr::Nearest)?;
        hwp.set_format(Format::s16())?;
        hwp.set_access(Access::RWInterleaved)?;
        pcm.hw_params(&hwp)?;
    }
    pcm.start()?;
    Ok(pcm)
}

#[cfg(not(target_os = "linux"))]
pub fn spawn_capture(cfg: crate::config::AudioConfig, _tx: broadcast::Sender<AudioPacket>) {
    tracing::warn!(device = %cfg.device, "audio capture is linux-only; disabled on this platform");
}
