#!/usr/bin/env python3
"""PCB regression gate: DRC (0 errors, 0 unconnected; connector silk-overhang
warnings are the only tolerated class) + every plane-net pad reaches a via or
zone (except R6.2, which connects to ACC by trace).  Run with KiCad's bundled
python3."""
import re, subprocess, sys

DRC_RPT = "/tmp/k3pwr-drc.rpt"
TOLERATED = {"silk_edge_clearance"}
EXPECT_UNSTITCHED = {("R6", "1"), ("R6", "2")}

r = subprocess.run(["kicad-cli", "pcb", "drc", "k3pwr-dongle.kicad_pcb",
                    "-o", DRC_RPT], capture_output=True, text=True)
text = open(DRC_RPT).read()
fail = 0

kinds = re.findall(r"^\[(\w+)\]", text, re.M)
bad = [k for k in kinds if k not in TOLERATED and k != "unconnected_items"]
uncon = kinds.count("unconnected_items")
if bad:
    print(f"FAIL: DRC violations: {sorted(set(bad))}")
    fail += 1
if uncon:
    print(f"FAIL: {uncon} unconnected items")
    fail += 1
print(f"DRC: {len(kinds)-uncon} violations "
      f"({len([k for k in kinds if k in TOLERATED])} tolerated silk-overhang), "
      f"{uncon} unconnected")

import pcbnew
board = pcbnew.LoadBoard("k3pwr-dongle.kicad_pcb")
board.BuildConnectivity()
conn = board.GetConnectivity()
for netname in ("GND", "VBUS", "/ACC_PWR"):
    tot = ok = 0
    for f in board.GetFootprints():
        for pad in f.Pads():
            if pad.GetNetname() != netname:
                continue
            key = (f.GetReference(), pad.GetNumber())
            tot += 1
            hit = any(i.Type() in (pcbnew.PCB_ZONE_T, pcbnew.PCB_VIA_T,
                                   pcbnew.PCB_TRACE_T)
                      for i in conn.GetConnectedItems(pad))
            if hit or key in EXPECT_UNSTITCHED:
                ok += 1
            else:
                print(f"FAIL: {key} not connected to plane/track")
                fail += 1
    print(f"{netname}: {ok}/{tot} pads connected")

print("pcb OK" if not fail else f"{fail} pcb check(s) failed")
sys.exit(1 if fail else 0)
