#!/usr/bin/env python3
"""Analytic verification of the k3pwr power-on chain (DTR# -> Q1 inverter ->
R2/C4/R3 gate filter -> Q2 open-drain -> K3 ACC pin 8).

Pass criteria (hard):
  - turn-on delay within 50ms..600ms across 2N7002 Vgs(th) corners (1.0-2.5V)
  - release within 0.3s..2s across corners
  - a single DTR glitch of <= 50ms never fires Q2, even at Vth(min)=1.0V
Reported (informational, accepted for rev 0.1 pending bench data):
  - repeated rapid DTR pulses can ratchet the gate up (3x50ms/200ms gaps ->
    ~1.45V, above a worst-case 1.0V-threshold part).  Realistic enumeration
    produces 1-2 pulses of <20ms; sustained assertion (e.g. ModemManager
    holding the port open) turns the radio on by design and must be handled
    in software (udev ignore rule on the server host).
"""
import math, sys

VBUS, R2, R3, C4 = 5.0, 33e3, 100e3, 10e-6
VF = VBUS * R3 / (R2 + R3)
TAU_C = (R2 * R3 / (R2 + R3)) * C4
TAU_D = R3 * C4
fail = 0

print(f"gate drive {VF:.2f}V, charge tau {TAU_C*1e3:.0f}ms, discharge tau {TAU_D*1e3:.0f}ms")
for vth in (1.0, 1.6, 2.5):
    t_on = -TAU_C * math.log(1 - vth / VF)
    t_off = TAU_D * math.log(VF / vth)
    ok_on, ok_off = 0.05 <= t_on <= 0.6, 0.3 <= t_off <= 2.0
    fail += (not ok_on) + (not ok_off)
    print(f"Vth {vth}V: turn-on {t_on*1e3:4.0f}ms {'OK' if ok_on else 'FAIL'} | "
          f"release {t_off*1e3:4.0f}ms {'OK' if ok_off else 'FAIL'}")

v50 = VF * (1 - math.exp(-0.050 / TAU_C))
ok = v50 < 1.0
fail += not ok
print(f"single 50ms glitch -> gate {v50:.2f}V vs worst-case Vth 1.0V  {'OK' if ok else 'FAIL'}")

v = 0.0
for _ in range(3):
    v = VF + (v - VF) * math.exp(-0.050 / TAU_C)
    peak = v
    v *= math.exp(-0.200 / TAU_D)
print(f"note: 3x50ms pulses @200ms gaps ratchet gate to {peak:.2f}V "
      f"(exceeds a 1.0V-Vth part; accepted for rev 0.1 - see docstring)")

print("sim OK" if not fail else f"{fail} sim check(s) FAILED")
sys.exit(1 if fail else 0)
