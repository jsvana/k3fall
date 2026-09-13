#!/usr/bin/env python3
"""Verify k3pwr-dongle connectivity: export a netlist with kicad-cli and
compare against the intended nets. Named nets must match by name; wire-only
nets (Q1B, LED_A) are matched by membership regardless of auto-name."""
import re, subprocess, sys

EXPECTED = {
    "VBUS_IN": {("J1", "A4"), ("J1", "A9"), ("J1", "B4"), ("J1", "B9"),
                ("F1", "1")},
    "VBUS_F": {("F1", "2"), ("D2", "2")},
    "VBUS": {("D2", "1"), ("C3", "1"), ("U2", "5"), ("U1", "16"), ("C1", "1"),
             ("R4", "1"), ("Q1", "2"), ("R5", "1"), ("R7", "2")},
    "GND": {("J1", "A1"), ("J1", "A12"), ("J1", "B1"), ("J1", "B12"),
            ("J1", "SH"), ("R8", "2"), ("R9", "2"),
            ("U2", "2"), ("C3", "2"), ("C1", "2"),
            ("U1", "15"), ("U1", "1"), ("C2", "2"), ("C4", "2"), ("R3", "2"),
            ("Q2", "2"), ("D3", "2"), ("J2", "5"), ("D1", "1")},
    "UD-_IN": {("J1", "A7"), ("J1", "B7"), ("U2", "1")},
    "UD+_IN": {("J1", "A6"), ("J1", "B6"), ("U2", "3")},
    "UD-": {("U2", "6"), ("U1", "6")},
    "UD+": {("U2", "4"), ("U1", "5")},
    "V3": {("U1", "4"), ("C2", "1")},
    "LOOP": {("U1", "2"), ("U1", "3"), ("D4", "1")},
    "DTR_N": {("U1", "13"), ("R1", "1")},
    "GATE_PRE": {("Q1", "3"), ("R2", "1")},
    "GATE": {("R2", "2"), ("C4", "1"), ("R3", "1"), ("Q2", "1")},
    "PWR_SINK": {("Q2", "3"), ("R6", "1")},
    "ACC_PWR": {("R6", "2"), ("D3", "1"), ("J2", "8")},
    None: [  # wire-only nets, matched by membership
        {("R1", "2"), ("R4", "2"), ("Q1", "1")},   # Q1B
        {("R5", "2"), ("D1", "2")},                # LED_A
        {("R7", "1"), ("D4", "2")},                # data LED anode
        {("J1", "A5"), ("R8", "1")},               # CC1 pulldown
        {("J1", "B5"), ("R9", "1")},               # CC2 pulldown
    ],
}

out = "/tmp/k3pwr-dongle.net"
subprocess.run(["kicad-cli", "sch", "export", "netlist",
                "k3pwr-dongle.kicad_sch", "-o", out],
               check=True, capture_output=True)
text = open(out).read()

nets = {}
for m in re.finditer(r'\(net\s+\(code "\d+"\)\s+\(name "([^"]*)"\)(.*?)(?=\(net\s+\(code |\Z)',
                     text, re.S):
    name = m.group(1).lstrip("/")
    nodes = {(r, p) for r, p in
             re.findall(r'\(node\s+\(ref "([^"]+)"\)\s+\(pin "([^"]+)"\)', m.group(2))
             if not r.startswith("#")}
    if nodes:
        nets[name] = nodes

fail = 0
for name, want in EXPECTED.items():
    if name is None:
        for members in want:
            if members not in nets.values():
                print(f"FAIL: no net with members {sorted(members)}")
                fail += 1
        continue
    got = nets.get(name)
    if got != want:
        print(f"FAIL: net {name}\n  want {sorted(want)}\n  got  {sorted(got) if got else got}")
        fail += 1

named = {k for k in nets
         if not k.startswith("Net-") and not k.startswith("unconnected-")}
extra = named - {k for k in EXPECTED if k}
if extra:
    print(f"FAIL: unexpected named nets: {sorted(extra)}")
    fail += 1

print("nets OK" if not fail else f"{fail} net check(s) failed")
sys.exit(1 if fail else 0)
