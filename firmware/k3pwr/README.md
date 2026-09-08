# k3pwr — remote K3 power control

Rust (ESP-IDF, std) firmware for the **Waveshare ESP32-P4-Module-DEV-KIT**.
Replaces the RemoteRig for K3 power-on: the board sits on the shack LAN via
its RJ45 Ethernet jack, and the k3fall server's PWR button drives it.

## How it works

- Ethernet (IP101GRI, RMII) → DHCP → announces itself as `k3pwr.local`.
- HTTP API on port 80:
  - `GET /power` → `{"on":bool}`
  - `POST /power` with `{"on":true|false}`
  - `GET /health`
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

## Ethernet pins (board-fixed)

IP101GRI on RMII: MDC=GPIO31, MDIO=GPIO52, PHY RST=GPIO51, REF_CLK in on
GPIO50 (50 MHz from the PHY), RXD0/1=GPIO29/30, CRS_DV=GPIO28,
TXD0/1=GPIO34/35, TX_EN=GPIO49. GPIO4 for the power stage stays clear of
all of these.

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
url = "http://k3pwr.local"
```

Restart k3fall; the PWR chip appears functional in the web UI topbar.
