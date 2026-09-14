#!/usr/bin/env python3
"""Generate the initial k3pwr-dongle PCB (run with KiCad's bundled python3:
/Applications/KiCad/KiCad.app/Contents/Frameworks/Python.framework/Versions/Current/bin/python3).

Board: 68x32mm 4-layer, JLCPCB JLC7628 stackup assumed.  Layers:
F.Cu signals / In1.Cu solid GND / In2.Cu split power (VBUS left, ACC_PWR
under the connector end) / B.Cu GND pour.  USB pigtail pads at the left edge, DE-15HD plug
off the right edge; placement follows schematic signal flow left->right.
Mounting: 2x M3 at the left corners, 1x M3 mid-board, plus the DSUB shell
holes (3.2mm, 25mm apart) at the right.  Pads get nets from the exported
netlist so "Update PCB from Schematic" matches cleanly; LCSC fields are
carried onto footprints for JLC assembly.  Signal routing is left to the
ratsnest — this is placement + planes only.  NOTE: the live board
has hand edits (LED row, silk ref nudges); prefer in-place scripts over
full regeneration."""
import re, subprocess, sys, os

import pcbnew
from pcbnew import FromMM, VECTOR2I

FPDIR = "/Applications/KiCad/KiCad.app/Contents/SharedSupport/footprints"
HERE = os.path.dirname(os.path.abspath(__file__))
SCH = os.path.join(HERE, "k3pwr-dongle.kicad_sch")
OUT = os.path.join(HERE, "k3pwr-dongle.kicad_pcb")
NETFILE = "/tmp/k3pwr-dongle-pcb.net"


def mm(x, y):
    return VECTOR2I(FromMM(x), FromMM(y))


# --- Parse netlist: ref -> footprint/value/LCSC, (ref,pad) -> net ----------
subprocess.run(["kicad-cli", "sch", "export", "netlist", SCH, "-o", NETFILE],
               check=True, capture_output=True)
text = open(NETFILE).read()

comps = {}   # ref -> (footprint, value, lcsc)
for m in re.finditer(r'\(comp\s+\(ref "([^"]+)"\)(.*?)(?=\(comp\s+\(ref |\(libparts)', text, re.S):
    ref, blob = m.group(1), m.group(2)
    if ref.startswith("#"):
        continue
    fp = re.search(r'\(footprint "([^"]+)"\)', blob)
    val = re.search(r'\(value "([^"]*)"\)', blob)
    lcsc = re.search(r'\(field \(name "LCSC"\)\s*"([^"]*)"', blob)
    comps[ref] = (fp.group(1) if fp else None,
                  val.group(1) if val else "",
                  lcsc.group(1) if lcsc else "")

padnet = {}  # (ref, pad) -> netname
netnames = set()
for m in re.finditer(r'\(net\s+\(code "\d+"\)\s+\(name "([^"]*)"\)(.*?)(?=\(net\s+\(code |\Z)', text, re.S):
    name = m.group(1)
    netnames.add(name)
    for r, p in re.findall(r'\(node\s+\(ref "([^"]+)"\)\s+\(pin "([^"]+)"\)', m.group(2)):
        if not r.startswith("#"):
            padnet[(r, p)] = name

# --- Board -----------------------------------------------------------------
board = pcbnew.BOARD()
bds = board.GetDesignSettings()
bds.SetCopperLayerCount(4)
# JLCPCB 4-layer conservative rules
bds.m_TrackMinWidth = FromMM(0.15)
bds.m_ViasMinSize = FromMM(0.45)
bds.m_MinThroughDrill = FromMM(0.2)
bds.m_MinClearance = FromMM(0.15)

nets = {}
for name in sorted(netnames):
    ni = pcbnew.NETINFO_ITEM(board, name)
    board.Add(ni)
    nets[name] = ni

# Board outline: (BX0,BY0)-(BX1,BY1)
BX0, BY0, BX1, BY1 = 20.0, 20.0, 88.0, 52.0
MIDY = (BY0 + BY1) / 2

for a, b in [((BX0, BY0), (BX1, BY0)), ((BX1, BY0), (BX1, BY1)),
             ((BX1, BY1), (BX0, BY1)), ((BX0, BY1), (BX0, BY0))]:
    seg = pcbnew.PCB_SHAPE(board)
    seg.SetShape(pcbnew.SHAPE_T_SEGMENT)
    seg.SetStart(mm(*a)); seg.SetEnd(mm(*b))
    seg.SetLayer(pcbnew.Edge_Cuts)
    seg.SetWidth(FromMM(0.1))
    board.Add(seg)

# --- Placement: ref -> (x, y, rot_deg) -------------------------------------
# Signal flow left->right: pigtail | USB protection | CH340 | logic | ACC out
PLACE = {
    "J1": (23.6, 36.0, 270),    # USB-C, opening off the left board edge
    "R8": (31.5, 30.5, 0),      # CC1 pulldown, next to connector
    "R9": (31.5, 41.5, 0),      # CC2 pulldown
    "F1": (33.0, 26.0, 0),
    "D2": (40.0, 26.0, 0),
    "C3": (46.5, 26.0, 0),
    "U2": (36.5, 33.5, 0),
    "R5": (33.175, 47.5, 0),
    "D1": (39.562, 48.5, 0),
    "U1": (46.0, 36.0, 0),
    "C1": (50.5, 29.5, 0),
    "C2": (40.5, 33.5, 90),
    "R7": (33.175, 49.5, 0),
    "D4": (43.938, 48.5, 0),
    "R1": (59.0, 32.55, 0),
    "R4": (61.563, 28.5, 90),
    "Q1": (62.5, 33.5, 0),
    "R2": (66.0, 33.5, 0),
    "C4": (67.0, 38.0, 90),
    "R3": (69.5, 38.0, 90),
    "Q2": (69.5, 33.5, 0),
    "R6": (73.0, 33.5, 0),
    "D3": (72.5, 42.0, 270),
    "J2": (77.11, 40.315, 90),  # edge line lands on BX1, shell centered
}

for ref, (fpname, value, lcsc) in sorted(comps.items()):
    lib, name = fpname.split(":")
    fp = pcbnew.FootprintLoad(f"{FPDIR}/{lib}.pretty", name)
    if fp is None:
        sys.exit(f"footprint not found: {fpname}")
    x, y, rot = PLACE[ref]
    fp.SetReference(ref)
    fp.SetValue(value)
    board.Add(fp)
    fp.SetPosition(mm(x, y))
    fp.SetOrientationDegrees(rot)
    if lcsc:
        fp.SetField("LCSC", lcsc)
        fld = fp.GetFieldByName("LCSC")
        if fld:
            fld.SetVisible(False)
    for pad in fp.Pads():
        num = pad.GetNumber()
        net = padnet.get((ref, num))
        if net is None and ref == "J2" and num == "SH":
            net = "GND"          # shell to chassis/logic ground
        if net:
            pad.SetNet(nets[net])

# --- Mounting holes --------------------------------------------------------
for href, hx, hy in [("H1", 24.5, 24.5), ("H2", 24.5, 47.5), ("H3", 54.0, 36.0)]:
    fp = pcbnew.FootprintLoad(f"{FPDIR}/MountingHole.pretty", "MountingHole_3.2mm_M3")
    fp.SetReference(href)
    fp.SetValue("M3")
    board.Add(fp)
    fp.SetPosition(mm(hx, hy))
    fp.SetAttributes(pcbnew.FP_EXCLUDE_FROM_POS_FILES
                     | pcbnew.FP_EXCLUDE_FROM_BOM | pcbnew.FP_BOARD_ONLY)
    fp.Reference().SetVisible(False)

# --- Plane stitching: via + fanout next to every SMD pad on a plane net ----
PLANE_NETS = {"GND", "VBUS", "/ACC_PWR"}
seen_vias = set()
for fp in board.GetFootprints():
    c = fp.GetPosition()
    for pad in fp.Pads():
        net = pad.GetNetname()
        if net not in PLANE_NETS or pad.HasHole():
            continue
        p = pad.GetPosition()
        if (p.x, p.y) in seen_vias:      # stacked pads (USB-C A/B rows)
            continue
        seen_vias.add((p.x, p.y))
        dx, dy = p.x - c.x, p.y - c.y    # fan out away from the body
        if len(fp.Pads()) > 2:
            # ICs: escape along the pad's long axis (perpendicular to the row)
            bb = pad.GetBoundingBox()
            horiz = bb.GetWidth() >= bb.GetHeight()
        else:
            horiz = abs(dx) >= abs(dy)   # passives: along the part axis
        if horiz:
            vpos = VECTOR2I(p.x + (FromMM(1.2) if dx >= 0 else -FromMM(1.2)), p.y)
        else:
            vpos = VECTOR2I(p.x, p.y + (FromMM(1.2) if dy >= 0 else -FromMM(1.2)))
        via = pcbnew.PCB_VIA(board)
        via.SetViaType(pcbnew.VIATYPE_THROUGH)
        via.SetLayerPair(pcbnew.F_Cu, pcbnew.B_Cu)
        via.SetPosition(vpos)
        via.SetDrill(FromMM(0.3))
        via.SetWidth(FromMM(0.6))
        via.SetNet(pad.GetNet())
        board.Add(via)
        tr = pcbnew.PCB_TRACK(board)
        tr.SetStart(p); tr.SetEnd(vpos)
        tr.SetWidth(FromMM(0.3))
        tr.SetLayer(pcbnew.F_Cu)
        tr.SetNet(pad.GetNet())
        board.Add(tr)

# --- Inner planes: In1 = GND, In2 = VBUS -----------------------------------
def add_zone(layer, netname, x0, y0, x1, y1, thermal=False):
    zone = pcbnew.ZONE(board)
    zone.SetLayer(layer)
    zone.SetNet(nets[netname])
    zone.SetIsFilled(True)
    zone.SetPadConnection(pcbnew.ZONE_CONNECTION_THERMAL if thermal
                          else pcbnew.ZONE_CONNECTION_FULL)
    zone.SetIslandRemovalMode(pcbnew.ISLAND_REMOVAL_MODE_ALWAYS)
    zone.SetLocalClearance(FromMM(0.25))   # keep webs between DSUB pins alive
    zone.SetMinThickness(FromMM(0.2))
    outline = zone.Outline()
    outline.NewOutline()
    for px, py in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]:
        outline.Append(FromMM(px), FromMM(py))
    board.Add(zone)
    return zone

# In1: solid ground plane.  In2: split power plane, VBUS under the USB /
# logic section, ACC_PWR under the connector end.  B.Cu: second ground pour
# (thermal spokes so the through-hole parts stay hand-solderable).
add_zone(pcbnew.In1_Cu, "GND", BX0, BY0, BX1, BY1)
add_zone(pcbnew.In2_Cu, "VBUS", BX0, BY0, 69.5, BY1)
add_zone(pcbnew.In2_Cu, "/ACC_PWR", 70.0, BY0, BX1, BY1)
add_zone(pcbnew.B_Cu, "GND", BX0, BY0, BX1, BY1, thermal=True)

filler = pcbnew.ZONE_FILLER(board)
filler.Fill(board.Zones())

# --- Silk annotations ------------------------------------------------------
def silk(textstr, x, y, layer=pcbnew.F_SilkS, size=1.0):
    t = pcbnew.PCB_TEXT(board)
    t.SetText(textstr)
    t.SetPosition(mm(x, y))
    t.SetLayer(layer)
    t.SetTextSize(VECTOR2I(FromMM(size), FromMM(size)))
    t.SetTextThickness(FromMM(0.15))
    if layer in (pcbnew.B_SilkS, pcbnew.B_Cu):
        t.SetMirrored(True)
    board.Add(t)

silk("k3pwr", 27.5, 22.0, size=1.5)
silk("W6JY", 35.0, 22.0)
silk("K3 ACC", 69.0, 23.0)
silk("USB", 25.0, 44.5)
silk("JLCJLCJLCJLC", 54.0, 46.0, layer=pcbnew.B_SilkS)  # JLC order number spot

pcbnew.SaveBoard(OUT, board)

# Report: sanity-check J2 pad positions and net counts
j2 = board.FindFootprintByReference("J2")
p8 = [p for p in j2.Pads() if p.GetNumber() == "8"][0]
print(f"wrote {OUT}: {len(list(board.GetFootprints()))} footprints, "
      f"{board.GetNetCount()} nets")
print(f"J2 pad8 at ({pcbnew.ToMM(p8.GetPosition().x):.2f}, "
      f"{pcbnew.ToMM(p8.GetPosition().y):.2f}) net {p8.GetNetname()}")
