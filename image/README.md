# k3fall Pi 5 SD image

Headless Raspberry Pi OS Lite (trixie, arm64) with everything baked in —
no internet required on the Pi for k3fall itself.

## What's inside

| Component | Detail |
|---|---|
| Base | Raspberry Pi OS Lite arm64 (2026-06-18, trixie), no GUI |
| SSH | Enabled on first boot |
| User | `jsvana` / password `k3fall` — **change it after first login** |
| Hostname | `k3fall` → reach it at `k3fall.local` (avahi) |
| k3fall | `/usr/local/bin/k3fall`, config `/etc/k3fall/config.toml`, service enabled → **http://k3fall.local:8080** |
| SDRplay | API 3.15 daemon (`sdrplay.service`) + udev rules + SoapySDRPlay3 module |
| ADS-B | readsb (rtlsdr) + tar1090 map → **http://k3fall.local/tar1090** |
| ADS-B feeds | adsb.fi + airplanes.live (anonymous), configured in `/etc/default/readsb` |

## Flash it

```
# Raspberry Pi Imager: Choose OS → Use custom → image/k3fall-pi5.img.xz
# (skip Imager's OS customization; user/ssh/hostname are already set)
# or:
xz -dk k3fall-pi5.img.xz
sudo dd if=k3fall-pi5.img of=/dev/rdiskN bs=4m status=progress
```

Ethernet is plug-and-play. For WiFi, use Imager's customization (it can
layer WiFi credentials on top) or `nmtui` over ethernet-connected SSH.

## First boot checklist

1. `ssh jsvana@k3fall.local` (password `k3fall`) → `passwd`
2. RSP2pro USB → K3 KXV3 IF OUT (BNC); USB-serial → K3 RS232 (38400, match K3 CONFIG:RS232)
3. RTL-SDR dongle + 1090 MHz antenna for ADS-B
4. Waterfall: http://k3fall.local:8080 — flip any inverted band with the INV chip
5. ADS-B map: http://k3fall.local/tar1090
6. Set your location for ADS-B range/MLAT-readiness: edit `--lat/--lon` into
   `RECEIVER_OPTIONS` in `/etc/default/readsb`, `sudo systemctl restart readsb`
7. FlightAware/FR24 feeding needs their account-based clients (not baked in);
   adsb.fi + airplanes.live feed anonymously out of the box — remove
   `FEED_OPTIONS` in `/etc/default/readsb` to stop.

## Rebuild the image

`./build-image.sh` (needs Docker; on Apple Silicon the arm64 chroot runs
natively — colima works: `colima start`). Output lands in this directory.
The k3fall binary is compiled from the working tree during the build.
