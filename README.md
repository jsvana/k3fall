# k3fall

K3 IF waterfall server: captures the K3's IF output via RSP2pro SDR, streams real-time spectrograms to a web browser, provides console UI for dial tracking and CAT click-to-tune frequency control, all running on a Raspberry Pi 5.

## Quick Start (Dev Mode)

Any machine with Rust installed can run k3fall in dev mode without hardware:

```bash
cargo run
```

Then open http://localhost:8080 in your browser to see the web UI and console.

## Raspberry Pi 5 Setup (One-Time)

### 1. Install SDRplay API v3 (aarch64)

Download the interactive installer from [SDRplay](https://www.sdrplay.com/api/):

```bash
cd ~/Downloads
chmod +x SDRplay_RSP_API-Linux-3.x.x.run
./SDRplay_RSP_API-Linux-3.x.x.run
```

Follow the installer prompts; it will install to `/usr/local/include/` and `/usr/local/lib/`.

### 2. Install SoapySDR and SoapySDRPlay3

On recent Raspberry Pi OS / Debian:

```bash
sudo apt install soapysdr-module-sdrplay3 soapysdr-tools
```

If the package is not available, build from source following SoapySDR documentation.

Verify the driver is installed:

```bash
SoapySDRUtil --probe=driver=sdrplay
```

### 3. Create k3fall User

Add a system user with access to USB devices:

```bash
sudo useradd -r -s /usr/sbin/nologin k3fall
sudo usermod -a -G dialout,plugdev k3fall
```

### 4. Wire Hardware

- **RSP2pro → K3 IF OUT**: Connect RSP2pro USB to Raspberry Pi; wire RSP2pro Tuner 1 SMA to K3's KXV3 IF OUT connector.
- **K3 serial (CAT) → Raspberry Pi**: Connect K3's RS-232 output (via USB-serial adapter) to Raspberry Pi USB.
  - K3 CAT speed: 38400 baud (set in K3 `CONFIG:RS232`).
  - Map the device in `config.toml` (typically `/dev/ttyUSB0` or `/dev/ttyUSB1`).

## Configuration

Copy the example config and edit as needed:

```bash
sudo cp config.example.toml /etc/k3fall/config.toml
sudo nano /etc/k3fall/config.toml
```

Config sections:

- **`[sdr]`**: Driver, center frequency (Hz), sample rate (Hz), gain (dB), antenna port.
  - `driver = "sdrplay"` for RSP2pro; `"file"` for dev-mode tone generation.
  - `center_hz`, `sample_rate`: Set to match your IF strip (e.g., 8215 kHz for K3's 9 MHz IF).
  - `gain_db`: Typical 35–45 dB; adjust for your signal strength.

- **`[cat]`**: K3 CAT interface.
  - `enabled`: Set to `true` on Pi; `false` for dev mode.
  - `device`: Serial port (e.g., `/dev/ttyUSB0`).
  - `baud`: 38400 (K3 default).

- **`[dsp]`**: DSP parameters.
  - `fft_size`: 32768 gives ~8 Hz per bin at 250 kSps; increase for finer resolution.
  - `rows_per_sec`: 30 = one spectrum row every ~33 ms.
  - `ref_level_db`, `range_db`: Set waterfall brightness and contrast; adjust for visibility.

- **`[[bands]]`**: Define amateur bands.
  - `name`, `low_hz`, `high_hz`: Band name and edges.
  - `direction`: `+1` (normal) or `-1` (inverted) depending on K3 injection side. Start with the example; use UI INV chip to verify, then persist the correct value.

- **`[server]`**: Web server listen address and port.

## Build Prerequisites (Pi)

`make deploy` builds on the Pi itself with `--features soapy`, which needs a Rust toolchain and several native dev packages beyond the runtime SoapySDR/SDRplay packages installed above (those provide the *libraries*, not the headers/build tooling `cargo build` needs to link against them).

Install Rust (if not already present):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Install native build dependencies:

```bash
sudo apt install build-essential pkg-config libudev-dev libclang-dev libsoapysdr-dev
```

- `build-essential`, `pkg-config`: standard toolchain needed by most `-sys` crates.
- `libudev-dev`: required by the `serialport` crate (K3 CAT serial port).
- `libclang-dev`, `libsoapysdr-dev`: required by `soapysdr-sys`'s bindgen step — `libsoapysdr-dev` provides the SoapySDR *headers*, which are not included in the `soapysdr-module-sdrplay3`/`soapysdr-tools` runtime packages installed in step 2 above.

## Deploy to Raspberry Pi

Run from your dev machine with k3fall source and SSH access to the Pi:

```bash
make deploy PI=<pi-hostname-or-ip>
```

The deploying user must have sudo rights on the Pi (with or without password). If using a password, the prompt will appear during deployment. To avoid password prompts, configure passwordless sudo for the deploying user on the Pi (optional).

This will:

1. Rsync source code to `<pi>:~/k3fall-src` (excluding target, .git, .superpowers, mockups).
2. SSH into the Pi and build with `cargo build --release --features soapy`.
3. Install the binary to `/usr/local/bin/k3fall`.
4. Create `/etc/k3fall/` and preserve existing `config.toml`.
5. Install systemd unit and enable/restart the service.

Once deployed, the k3fall service starts automatically on boot and restarts on failure.

## Hardware Verification

After deployment, verify the setup:

- **Waterfall displays band activity**: Open http://pi-address:8080 and confirm signal activity on the waterfall.
- **Dial tracks**: Click in the frequency dial area; the marker should follow your click.
- **Click-to-tune moves K3**: Click a frequency on the waterfall; the K3 should QSY to that frequency.
- **Inversion per band**: Check that spectrum orientation matches your expectation (LSB on left / USB on right). Use the UI INV chip to flip if needed, then update `config.toml` `direction` field.
- **USB recovery**: Disconnect and reconnect the RSP2pro USB and K3 serial; the service should reconnect and resume operation within seconds.

## Development

Build locally:

```bash
cargo build
```

Run tests:

```bash
cargo test
```

Format and lint:

```bash
make quality
```

(or `cargo fmt` and `cargo clippy -- -D warnings` individually)
