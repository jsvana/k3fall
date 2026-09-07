# k3fall — High-Definition Web Waterfall for the Elecraft K3 IF

**Date:** 2026-09-07
**Status:** Approved design, pre-implementation
**Hardware:** Elecraft K3 (KXV3 IF OUT) → SDRplay RSP2pro → Raspberry Pi 5

## Purpose

A Rust server running on a Raspberry Pi 5 that digitizes the K3's
8.215 MHz first-IF output with an RSP2pro and serves a high-definition,
CAT-tracked spectrum waterfall to any browser on the LAN. Clicking the
waterfall tunes the K3.

Effectively: a P3 panadapter with ~100× the frequency resolution,
viewable from anywhere in the house.

## Signal Chain

- The KXV3 IF OUT taps the K3's first IF at **8.215 MHz** with roughly
  ±100 kHz of usable spectrum around the VFO (same tap the P3 uses).
- RSP2pro tuned to 8.215 MHz, **200 kHz IF bandwidth**, **250 kS/s**
  complex samples via the SDRplay API v3.
- Access is through **SoapySDR + SoapySDRPlay3** (both packaged for
  aarch64). The proprietary `sdrplay_api` daemon is a Pi setup
  prerequisite, not a build dependency.
- Fallback if 250 kS/s decimation misbehaves on the RSP2pro: run at
  2 MS/s and decimate ×8 in Rust with a FIR (documented, behind config).

## Architecture

Single Rust binary (axum web server + DSP threads) plus a static
single-page client. No database, no external services.

```
RSP2pro ──SoapySDR──▶ IqSource ──▶ DSP thread ──rows──▶ broadcast ──▶ WebSocket ──▶ browser (WebGL2)
K3 serial ──────────▶ CAT task ──vfo/mode────────────▶ broadcast ──▶ (same WS, JSON frames)
browser click ──WS──▶ CAT task ──FA cmd──▶ K3
```

### Components

1. **`IqSource` trait** — `read(&mut self, buf: &mut [Complex<f32>])`
   plus tune/sample-rate metadata. Two implementations:
   - `SoapySource` — real hardware.
   - `FileSource` — IQ file playback (loops), enabling all DSP/UI
     development and tests on macOS with no hardware attached.

2. **DSP thread** — Hann-windowed **32,768-point FFT** (rustfft) with
   overlap tuned to produce **30 rows/sec** at 250 kS/s
   (**~7.6 Hz/bin**). Each row: power in dB, quantized to u8 against an
   adjustable reference level and dynamic range. Full-resolution rows go
   to an in-process broadcast channel.

3. **Per-client row shaping** — each WebSocket session declares its
   viewport width (≤ 4096 px) and visible span. The server slices the
   full-res row to the span and **max-pools** (peak-detect) down to the
   viewport width so narrow CW signals never disappear between pixels.
   Zoom is a re-slice of the same FFT — no retune, no re-FFT.

4. **CAT task** — owns the K3 serial port (`serialport` crate,
   Kenwood-style protocol). Enables `AI2;` so VFO/mode changes are
   pushed; falls back to polling `FA;`/`IF;` at 5 Hz if auto-info
   fails. Publishes `{vfo_a_hz, mode, band}` to clients. Accepts tune
   requests (see click-to-tune).

5. **Web layer** — axum. One HTTP route serving the embedded static
   client (rust-embed or include_dir), one WebSocket route.
   - **Binary frames** (waterfall rows): header `{seq: u32,
     center_rf_hz: u64, span_hz: u32, bins: u16, flags: u8}` then
     `bins` u8 values.
   - **JSON frames** (control): VFO updates, status (SDR ok / CAT ok),
     client settings (viewport width, span, ref/range), tune requests.

6. **Client** — single static page, no framework. WebGL2 waterfall
   using a scrolling texture ring buffer, spectrum line overlay,
   viridis colormap, drag-adjustable ref level / dynamic range,
   frequency scale bound to the CAT-reported VFO. Renders at
   `devicePixelRatio` for retina sharpness. Scroll/pinch to zoom,
   drag to pan within the captured span.

## Frequency Mapping & Band Inversion

The K3's first-LO injection side varies by band, so the IF spectrum is
inverted on some bands. Mapping:

```
display_rf = vfo_hz + direction(band) × (bin_hz − if_center_hz)
```

`direction(band) ∈ {+1, −1}` comes from a per-band table in the config
file. Ship best-known defaults; the UI has a **flip toggle** that
persists the correction per band, so any wrong default is fixed
empirically in seconds. The exact IF center offset relative to the
displayed VFO also varies slightly by mode (CW offset); a per-mode
trim in config handles this.

## Click-to-Tune (v1)

Clicking the waterfall/spectrum tunes the K3:

1. Client converts click x-position → RF Hz using the current mapping.
2. Sends `{type: "tune", freq_hz}` over the WebSocket.
3. CAT task rounds to 10 Hz and writes `FA...;` to the K3.
4. The resulting `AI2` VFO report updates every client's scale —
   the waterfall itself doesn't move (the IF tap follows the VFO), the
   scale re-labels.

Guards: tune requests are rejected with a visible error when CAT is
disconnected; requests outside the displayed span are ignored;
rate-limited to 10/sec.

## Error Handling

- **SDR USB drop:** bounded exponential-backoff reconnect loop; status
  surfaced in the UI banner. Process never exits on SDR loss.
- **CAT unplugged:** waterfall continues with an IF-relative scale and
  a "CAT disconnected" banner; click-to-tune disabled.
- **WebSocket backpressure:** per-client bounded queue; drop oldest
  rows rather than buffer unboundedly.
- **Serial contention:** the server owns the port exclusively; a
  config flag disables CAT entirely if the port is needed elsewhere.

## Configuration

Single TOML file: SDR settings (center, rate, gain, antenna port),
serial device + baud, per-band inversion table, per-mode IF trim,
listen address, defaults for ref/range/span.

## UI Direction (decided 2026-09-07)

Of the four explored mockups, the **console** direction won
(`mockups/console.html`): dark, keyboard-first instrument console — warm
near-black surfaces, one signal-orange accent, Archivo + Chivo Mono
typography, giant typographic frequency readout, waterfall as the hero
(~60% viewport), heat/ember colormap, dense spot table, telemetry rail.
The mockup is the visual contract for the client.

v1 client scope: readout row (VFO A/B, mode/filter/RIT chips), spectrum +
waterfall with click-to-tune and hover frequency chip, frequency ruler,
status/telemetry values that the server actually knows (SDR/CAT state,
FFT config, frame rate). The keyer panel, spot table, and ⌘K command bar
from the mockup are follow-ups, not v1.

Rendering note: v1 uses a 2D canvas (drawImage scroll + putImageData row
blit at devicePixelRatio) rather than WebGL2. At ≤4096 px × 30 rows/sec
this is comfortably GPU-composited; WebGL is an upgrade path only if
profiling demands it.

## Testing

- **DSP:** synthetic-tone IQ through `FileSource` → assert tone lands
  in the expected bin, at both `direction = ±1`; max-pool preserves a
  1-bin tone at every viewport width.
- **CAT:** parser tests against canned K3 response strings (FA, IF,
  AI2 async reports, error cases); tune-command formatting tests.
- **Protocol:** round-trip encode/decode of binary row frames.
- Quality gates: `cargo fmt` → `cargo clippy` → `cargo test` before
  every commit.

## Deployment

- `make deploy PI=<host>`: cross-compile (or build on the Pi) for
  aarch64, scp binary + config, install/restart a systemd unit.
- Pi prerequisites (documented in README, one-time): SDRplay API v3
  daemon (aarch64 installer), SoapySDR, SoapySDRPlay3, udev rules.
- Graduating to an Ansible role later is straightforward if it joins
  the managed fleet.

## Out of Scope (v1)

- Audio demodulation / listening in the browser.
- Multi-client independent sessions beyond span/zoom (all clients see
  the same SDR capture — inherent to the IF tap).
- IQ recording, averaging modes, peak-hold persistence.
- Auth/TLS (LAN-only service).
