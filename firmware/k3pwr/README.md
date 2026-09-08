# k3pwr — remote K3 power control

Rust (ESP-IDF, std) firmware for the **Waveshare ESP32-P4-Module-DEV-KIT**.
Replaces the RemoteRig for K3 power-on: the board plugs into the Pi over
USB (powered by it, enumerates as `/dev/ttyACM0` via the P4's
USB-Serial-JTAG), and the k3fall server's PWR button drives it.

## How it works

- Line protocol over the USB serial console:
  - `on\n` / `off\n` → sets the power line, replies `ok on` / `ok off`
  - `status\n` → replies with current state
- GPIO4 drives an external open-collector stage. **Level semantics** (same
  as RemoteRig OUT0 mode "Connect"): stage conducting → K3 ACC pin 8 held
  at ground → radio ON. Released → radio OFF. Boot state is OFF.

## Wiring — REQUIRED transistor stage

The K3's PWR ON line is pulled up above 3.3 V inside the radio, so the
GPIO must **never** touch it directly, and never drive it high. Use a
2N7000 (or any small N-MOSFET/NPN) as the open collector:

```
ESP32-P4 GPIO4 ──1kΩ──┬── 2N7000 gate
                      100kΩ
                       │
ESP32-P4 GND ──────────┴── 2N7000 source ───── RJ45 pin 8 (GND)
                           2N7000 drain ────── RJ45 pin 5 (OUT0)
```

The RJ45 female jack mates with the **RemoteRig E3c / Elecraft E850510**
cable's Radio-RRC end; its HD15 goes to the K3 ACC port:

- RJ45 pin 5 (OUT0, open collector) → ACC pin 8 (power on)
- RJ45 pin 8 (GND) → ACC pin 5 (GND)
- RJ45 pin 6 (OUT1 / FSK) — unused here

(Pinout per RRC-1258MkII manual rev A33, p. 210.)

The 100k gate pulldown guarantees the radio stays off while the ESP32
boots or is unpowered.

## Build & flash

Prereqs (one-time): `rustup toolchain install nightly --component rust-src`,
`cargo install ldproxy espflash`.

```
make build          # cargo build --release (downloads ESP-IDF on first run)
make flash          # espflash flash --monitor (board on USB)
make monitor        # serial logs
```

## k3fall server hookup

In `/etc/k3fall/config.toml`:

```toml
[power]
device = "/dev/ttyACM0"   # or a stable /dev/serial/by-id/... path
baud = 115200             # ignored by USB CDC, required by the API
```

Restart k3fall; the PWR chip appears functional in the web UI topbar.
