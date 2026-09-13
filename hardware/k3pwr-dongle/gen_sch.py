#!/usr/bin/env python3
"""Generate k3pwr-dongle.kicad_sch: CH340C DTR -> PNP inverter -> RC -> 2N7002
open-collector into K3 ACC pin 8 (DE-15HD), USB pigtail pads. Symbols are
extracted verbatim from the installed KiCad libraries. The schematic is fully
wired: components connect pin-to-pin within four boxed functional groups
(USB input/protection, USB-serial bridge, power-on logic, K3 ACC output),
GND/VBUS use power symbols, and only the nets that cross groups (VBUS, UD+,
UD-, DTR_N, PWR_SINK) use global labels. NC pins get no_connect markers.
Verify with check_nets.py after regenerating."""
import re, uuid

SYMDIR = "/Applications/KiCad/KiCad.app/Contents/SharedSupport/symbols"

def extract(lib, name):
    text = open(f"{SYMDIR}/{lib}.kicad_sym").read()
    key = f'(symbol "{name}"'
    i = text.index(key)
    depth = 0
    for j in range(i, len(text)):
        if text[j] == '(': depth += 1
        elif text[j] == ')':
            depth -= 1
            if depth == 0:
                return text[i:j+1]
    raise ValueError(name)

def blocks(text, key):
    """Yield paren-balanced blocks starting with key."""
    start = 0
    while True:
        i = text.find(key, start)
        if i < 0:
            return
        depth = 0
        for j in range(i, len(text)):
            if text[j] == '(': depth += 1
            elif text[j] == ')':
                depth -= 1
                if depth == 0:
                    yield text[i:j+1]
                    start = j
                    break
        else:
            return

def pins(sym_sexp):
    """[(number, name, x, y)] from a symbol definition (symbol coords, y-up)."""
    out = []
    for blk in blocks(sym_sexp, '(pin '):
        at = re.search(r'\(at\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)\)', blk)
        name = re.search(r'\(name\s+"([^"]*)"', blk)
        num = re.search(r'\(number\s+"([^"]*)"', blk)
        if at and name and num:
            out.append((num.group(1), name.group(1), float(at.group(1)), float(at.group(2))))
    return out

LIBS = [
    ("Connector_Generic", "Conn_01x05"),
    ("Interface_USB", "CH340C"),
    ("Device", "R"),
    ("Device", "C"),
    ("Device", "LED"),
    ("Transistor_BJT", "Q_PNP_BEC"),
    ("Transistor_FET", "Q_NMOS_GSD"),
    ("Connector", "DE15_Pins_HighDensity"),
    ("power", "PWR_FLAG"),
    ("power", "GND"),
    ("power", "VBUS"),
    ("Power_Protection", "USBLC6-2P6"),
    ("Device", "Polyfuse"),
    ("Device", "D_Schottky"),
    ("Device", "D_TVS"),
]

# Prefix parent symbol names with lib name (KiCad embeds as "LIB:NAME").
lib_symbols = []
PIN_SOURCE = {}
seen = set()
for lib, name in LIBS:
    s = extract(lib, name)
    ext = re.search(r'\(extends "([^"]+)"\)', s)
    if ext:
        parent = ext.group(1)
        ps = extract(lib, parent)
        if f"{lib}:{parent}" not in seen:
            seen.add(f"{lib}:{parent}")
            lib_symbols.append(ps.replace(f'(symbol "{parent}"', f'(symbol "{lib}:{parent}"', 1))
        s = s.replace(f'(extends "{parent}")', f'(extends "{lib}:{parent}")')
        PIN_SOURCE[f"{lib}:{name}"] = (lib, parent)
    else:
        PIN_SOURCE[f"{lib}:{name}"] = (lib, name)
    if f"{lib}:{name}" not in seen:
        seen.add(f"{lib}:{name}")
        lib_symbols.append(s.replace(f'(symbol "{name}"', f'(symbol "{lib}:{name}"', 1))

sym_pins = {f"{lib}:{name}": pins(extract(*PIN_SOURCE[f"{lib}:{name}"])) for lib, name in LIBS}

def U(): return str(uuid.uuid4())

def g(v):
    """Format a coordinate without float noise."""
    return f"{round(v, 3):g}"

# ---------------------------------------------------------------------------
# Placement.  All coordinates are absolute sheet mm (y down), pins on the
# 1.27 grid.  rot is degrees CCW on screen; mir is None|'x'|'y'.
# ---------------------------------------------------------------------------
SYMS = {}    # ref -> (lib, name, value, x, y, rot, mir)
ORDER = []

def place(ref, lib, name, value, x, y, rot=0, mir=None):
    SYMS[ref] = (lib, name, value, x, y, rot, mir)
    ORDER.append(ref)

def xf(sx, sy, rot, mir):
    """Symbol-space pin offset -> screen offset for a placed instance."""
    dx, dy = sx, -sy
    if mir == 'x': dy = -dy
    if mir == 'y': dx = -dx
    for _ in range(rot // 90):
        dx, dy = dy, -dx
    return dx, dy

used = set()  # (ref, pin_number) that the wiring plan connects

def P(ref, num):
    """Absolute position of a pin; records it as connected."""
    lib, name, _, x, y, rot, mir = SYMS[ref]
    for n, _, sx, sy in sym_pins[f"{lib}:{name}"]:
        if n == num:
            dx, dy = xf(sx, sy, rot, mir)
            used.add((ref, num))
            return (round(x + dx, 3), round(y + dy, 3))
    raise KeyError(f"{ref} pin {num}")

# --- Group A: USB input + protection ---------------------------------------
place("J1", "Connector_Generic", "Conn_01x05", "USB_pigtail", 30.48, 43.18, 0, 'y')
place("F1", "Device", "Polyfuse", "500mA", 55.88, 38.10, 90)
place("D2", "Device", "D_Schottky", "SS14", 73.66, 38.10, 180)
place("C3", "Device", "C", "4.7u", 101.60, 45.72)
place("U2", "Power_Protection", "USBLC6-2P6", "USBLC6-2SC6", 63.50, 53.34)
place("R5", "Device", "R", "1k", 30.48, 66.04)
place("D1", "Device", "LED", "green", 30.48, 76.20, 90)
# --- Group B: USB-serial bridge --------------------------------------------
place("U1", "Interface_USB", "CH340C", "CH340C", 187.96, 48.26)
place("C1", "Device", "C", "100n", 172.72, 33.02)
place("C2", "Device", "C", "100n", 215.90, 33.02)
place("D4", "Device", "LED", "yellow", 210.82, 46.99)
place("R7", "Device", "R", "1k", 222.25, 46.99, 90)
# --- Group C: power-on logic -----------------------------------------------
place("R1", "Device", "R", "4.7k", 36.83, 137.16, 90)
place("R4", "Device", "R", "10k", 45.72, 127.00)
place("Q1", "Transistor_BJT", "Q_PNP_BEC", "MMBT3906", 55.88, 137.16, 0, 'x')
place("R2", "Device", "R", "33k", 68.58, 148.59, 90)
place("C4", "Device", "C", "10u", 80.01, 156.21)
place("R3", "Device", "R", "100k", 92.71, 156.21)
place("Q2", "Transistor_FET", "Q_NMOS_GSD", "2N7002", 105.41, 148.59)
# --- Group D: K3 ACC output ------------------------------------------------
place("R6", "Device", "R", "100R", 198.12, 134.62, 90)
place("D3", "Device", "D_TVS", "SMAJ15A", 210.82, 142.24, 270)
place("J2", "Connector", "DE15_Pins_HighDensity", "K3_ACC", 233.68, 134.62)

# Power symbols / flags: (ref, lib_name, x, y)
POWER = []
_pwr_n = 0
def gnd(x, y):
    global _pwr_n
    _pwr_n += 1
    POWER.append((f"#PWR{_pwr_n:02d}", "GND", x, y))
def vbus(x, y):
    global _pwr_n
    _pwr_n += 1
    POWER.append((f"#PWR{_pwr_n:02d}", "VBUS", x, y))
def flag(n, x, y):
    POWER.append((f"#FLG{n:02d}", "PWR_FLAG", x, y))

WIRES = []      # ((x1,y1),(x2,y2))
JUNCTIONS = []  # (x,y)
GLABELS = []    # (net, x, y, rot)
LLABELS = []    # (net, x, y)

def wire(*pts):
    for a, b in zip(pts, pts[1:]):
        assert a[0] == b[0] or a[1] == b[1], f"non-orthogonal wire {a}-{b}"
        WIRES.append((a, b))

# --- Group A wiring: pigtail -> polyfuse -> reverse guard -> VBUS ----------
wire(P("J1", "1"), P("F1", "1"))                       # VBUS_IN
LLABELS.append(("VBUS_IN", 38.10, 38.10))
wire(P("F1", "2"), P("D2", "2"))                       # VBUS_F -> anode
LLABELS.append(("VBUS_F", 60.96, 38.10))
wire(P("D2", "1"), (101.60, 38.10), P("C3", "1"))      # cathode = VBUS rail
flag(1, 83.82, 38.10); JUNCTIONS.append((83.82, 38.10))
vbus(91.44, 38.10);    JUNCTIONS.append((91.44, 38.10))
wire(P("C3", "2"), (101.60, 52.07)); gnd(101.60, 52.07)
wire(P("J1", "4"), (38.10, 45.72), (38.10, 48.26))     # pigtail grounds
wire(P("J1", "5"), (38.10, 48.26)); JUNCTIONS.append((38.10, 48.26))
wire((38.10, 48.26), (38.10, 50.80)); gnd(38.10, 50.80)
# U2 is flow-through ESD: connector-side nets (UD-_IN/UD+_IN) stay separate
# from the device-side UD-/UD+ so layout is forced through the chip.
wire(P("J1", "2"), (46.99, 40.64), (46.99, 53.34), P("U2", "1"))
LLABELS.append(("UD-_IN", 38.10, 40.64))
wire(P("J1", "3"), (44.45, 43.18), (44.45, 55.88), P("U2", "3"))
LLABELS.append(("UD+_IN", 38.10, 43.18))
wire(P("U2", "5"), (63.50, 45.72)); vbus(63.50, 45.72)
wire(P("U2", "2"), (63.50, 63.50)); gnd(63.50, 63.50)
wire(P("U2", "6"), (78.74, 53.34)); GLABELS.append(("UD-", 78.74, 53.34, 0))
wire(P("U2", "4"), (78.74, 55.88)); GLABELS.append(("UD+", 78.74, 55.88, 0))
vbus(30.48, 59.69); wire((30.48, 59.69), P("R5", "1"))
wire(P("R5", "2"), P("D1", "2"))                       # LED_A
wire(P("D1", "1"), (30.48, 82.55)); gnd(30.48, 82.55)
flag(2, 110.49, 46.99); wire((110.49, 46.99), (110.49, 49.53)); gnd(110.49, 49.53)

# --- Group B wiring: CH340C ------------------------------------------------
wire(P("C1", "1"), (185.42, 29.21), P("U1", "16"))     # VCC rail + decoupling
vbus(179.07, 29.21); JUNCTIONS.append((179.07, 29.21))
wire(P("C1", "2"), (172.72, 40.64))
wire(P("U1", "15"), (172.72, 40.64))                   # R232 strap to GND
JUNCTIONS.append((172.72, 40.64))
wire((172.72, 40.64), (172.72, 43.18)); gnd(172.72, 43.18)
wire(P("U1", "4"), (187.96, 27.94), (215.90, 27.94), P("C2", "1"))  # V3
LLABELS.append(("V3", 196.85, 27.94))
wire(P("C2", "2"), (215.90, 39.37)); gnd(215.90, 39.37)
wire(P("U1", "1"), (187.96, 66.04)); gnd(187.96, 66.04)
wire(P("U1", "5"), (163.83, 45.72)); GLABELS.append(("UD+", 163.83, 45.72, 180))
wire(P("U1", "6"), (163.83, 48.26)); GLABELS.append(("UD-", 163.83, 48.26, 180))
wire(P("U1", "2"), (203.20, 38.10), (203.20, 40.64), P("U1", "3"))  # loopback
LLABELS.append(("LOOP", 203.20, 39.37))
# Data-activity LED: sinks into TXD (loopback), lights on the server's probe.
wire((203.20, 40.64), (203.20, 46.99), P("D4", "1"))
JUNCTIONS.append((203.20, 40.64))
wire(P("D4", "2"), P("R7", "1"))
wire(P("R7", "2"), (226.06, 44.45)); vbus(226.06, 44.45)
wire(P("U1", "13"), (209.55, 55.88)); GLABELS.append(("DTR_N", 209.55, 55.88, 0))

# --- Group C wiring: DTR# -> inverter -> RC delay -> switch ----------------
GLABELS.append(("DTR_N", 25.40, 137.16, 180))
wire((25.40, 137.16), P("R1", "1"))
wire(P("R1", "2"), P("Q1", "1"))                       # Q1B
wire(P("R4", "2"), (45.72, 137.16)); JUNCTIONS.append((45.72, 137.16))
wire(P("R4", "1"), (45.72, 120.65)); vbus(45.72, 120.65)
wire(P("Q1", "2"), (58.42, 127.00)); vbus(58.42, 127.00)
wire(P("Q1", "3"), (58.42, 148.59), P("R2", "1"))      # GATE_PRE
LLABELS.append(("GATE_PRE", 58.42, 146.05))
wire(P("R2", "2"), P("Q2", "1"))                       # GATE line
LLABELS.append(("GATE", 85.09, 148.59))
wire(P("C4", "1"), (80.01, 148.59)); JUNCTIONS.append((80.01, 148.59))
wire(P("C4", "2"), (80.01, 162.56)); gnd(80.01, 162.56)
wire(P("R3", "1"), (92.71, 148.59)); JUNCTIONS.append((92.71, 148.59))
wire(P("R3", "2"), (92.71, 162.56)); gnd(92.71, 162.56)
wire(P("Q2", "3"), (107.95, 134.62), (115.57, 134.62))
GLABELS.append(("PWR_SINK", 115.57, 134.62, 0))
wire(P("Q2", "2"), (107.95, 156.21)); gnd(107.95, 156.21)

# --- Group D wiring: limiter -> TVS -> DE-15HD -----------------------------
GLABELS.append(("PWR_SINK", 182.88, 134.62, 180))
wire((182.88, 134.62), P("R6", "1"))
wire(P("R6", "2"), P("J2", "8"))                       # ACC_PWR
LLABELS.append(("ACC_PWR", 204.47, 134.62))
wire(P("D3", "1"), (210.82, 134.62)); JUNCTIONS.append((210.82, 134.62))
wire(P("D3", "2"), (210.82, 148.59)); gnd(210.82, 148.59)
wire(P("J2", "5"), (215.90, 121.92), (215.90, 124.46)); gnd(215.90, 124.46)

# ---------------------------------------------------------------------------
GROUPS = [
    ("USB INPUT + PROTECTION", 12.70, 17.78, 121.92, 95.25),
    ("USB-SERIAL BRIDGE", 144.78, 17.78, 229.87, 81.28),
    ("POWER-ON LOGIC", 12.70, 110.49, 160.02, 186.69),
    ("K3 ACC OUTPUT", 165.10, 110.49, 284.48, 163.83),
]

NOTES = [
    (13.97, 27.94, "solder pads for USB-A pigtail to the Pi"),
    (13.97, 30.48, "(VBUS, D-, D+, GND, shield->GND)"),
    (49.53, 27.94, "protects the Pi USB port from"),
    (49.53, 30.48, "board or hood-wiring faults"),
    (80.01, 27.94, "reverse-polarity guard for a"),
    (80.01, 30.48, "miswired pigtail (~0.3V drop)"),
    (95.25, 58.42, "VBUS bulk reservoir"),
    (95.25, 60.96, "for pigtail inductance"),
    (49.53, 68.58, "IEC 61000-4-2 ESD clamp"),
    (49.53, 71.12, "for D+/D-/VBUS"),
    (13.97, 88.90, "power LED current limit (~3mA)"),
    (13.97, 91.44, "USB power indicator"),
    (147.32, 33.02, "CH340C VCC decoupling"),
    (147.32, 35.56, "(C1 at pin 16)"),
    (205.74, 60.96, "D4 blinks on serial data"),
    (205.74, 63.50, "(server loopback probe)"),
    (172.72, 71.12, "USB-serial bridge; only DTR# is used"),
    (172.72, 73.66, "TXD->RXD loopback lets the server"),
    (172.72, 76.20, "probe that the dongle is present"),
    (172.72, 80.01, "C2: V3 internal-regulator bypass (per CH340 datasheet)"),
    (16.51, 144.78, "Q1 base current limit"),
    (13.97, 147.32, "R4 holds Q1 off while DTR# floats"),
    (13.97, 149.86, "during USB enumeration"),
    (62.23, 128.27, "polarity inverter: DTR# is active-low,"),
    (62.23, 130.81, "idles high -> idle/unplugged = radio OFF"),
    (62.23, 141.61, "R2 with C4: ~0.3s turn-on delay"),
    (66.04, 168.91, "C4 gate glitch filter: with R2/R3 swallows"),
    (66.04, 171.45, "DTR pulses from USB enumeration"),
    (66.04, 173.99, "(~0.3s on, ~1s release)"),
    (66.04, 179.07, "R3 gate pulldown: guarantees OFF when"),
    (66.04, 181.61, "unpowered; ~1s release with C4"),
    (113.03, 146.05, "open-collector sink for ACC pin 8:"),
    (113.03, 148.59, "only ever pulls low, never sources"),
    (172.72, 124.46, "R6 limits fault current into ACC wiring;"),
    (172.72, 127.00, "negligible vs the K3 pull-up"),
    (186.69, 157.48, "D3 clamps ESD/spikes on the ACC line;"),
    (186.69, 160.02, "mount at connector, cathode to line"),
    (222.25, 153.67, "K3 ACC: pin 8 = PWR ON (sink = on),"),
    (222.25, 156.21, "pin 5 = GND, all others unused"),
]

# Footprint + JLCPCB LCSC part number per component (LCSC "" = hand-solder /
# not JLC-assembled).  Basic/Preferred parts chosen where available.
FPLCSC = {
    "J1": ("Connector_PinHeader_2.54mm:PinHeader_1x05_P2.54mm_Vertical", ""),
    "F1": ("Fuse:Fuse_1206_3216Metric", "C720075"),
    "D2": ("Diode_SMD:D_SMA", "C2480"),
    "C3": ("Capacitor_SMD:C_0805_2012Metric", "C1779"),
    "U2": ("Package_TO_SOT_SMD:SOT-23-6", "C2687116"),
    "R5": ("Resistor_SMD:R_0603_1608Metric", "C21190"),
    "D1": ("LED_SMD:LED_0805_2012Metric", "C2297"),
    "U1": ("Package_SO:SOIC-16_3.9x9.9mm_P1.27mm", "C84681"),
    "C1": ("Capacitor_SMD:C_0603_1608Metric", "C14663"),
    "C2": ("Capacitor_SMD:C_0603_1608Metric", "C14663"),
    "D4": ("LED_SMD:LED_0805_2012Metric", "C2296"),
    "R7": ("Resistor_SMD:R_0603_1608Metric", "C21190"),
    "R1": ("Resistor_SMD:R_0603_1608Metric", "C23162"),
    "R4": ("Resistor_SMD:R_0603_1608Metric", "C25804"),
    "Q1": ("Package_TO_SOT_SMD:SOT-23", "C7420354"),
    "R2": ("Resistor_SMD:R_0603_1608Metric", "C4216"),
    "C4": ("Capacitor_SMD:C_0805_2012Metric", "C15850"),
    "R3": ("Resistor_SMD:R_0603_1608Metric", "C25803"),
    "Q2": ("Package_TO_SOT_SMD:SOT-23", "C916396"),
    "R6": ("Resistor_SMD:R_0603_1608Metric", "C22775"),
    "D3": ("Diode_SMD:D_SMA", "C19077535"),
    "J2": ("Connector_Dsub:DSUB-15-HD_Pins_Horizontal_P2.29x2.54mm_EdgePinOffset8.35mm_Housed_MountingHolesOffset10.89mm", ""),
}

# Ref/Value text spots that the default (above/below symbol) rules would put
# on top of wires, power symbols, or neighbors.
OVERRIDES = {
    "J1": {"value": (24.13, 52.07)},
    "U2": {"ref": (68.58, 45.72), "value": (74.93, 60.96)},
    "C3": {"ref": (106.68, 43.18), "value": (106.68, 46.99)},
    "R5": {"ref": (25.40, 62.23), "value": (34.29, 67.31)},
    "D1": {"ref": (24.13, 73.66), "value": (36.83, 76.20)},
    "U1": {"value": (176.53, 66.04)},
    "C1": {"value": (166.37, 36.83)},
    "C2": {"value": (221.62, 33.02)},
    "R4": {"ref": (40.64, 120.65)},
    "Q1": {"value": (48.26, 144.78)},
    "C4": {"ref": (74.93, 153.67), "value": (85.09, 158.75)},
    "R3": {"ref": (87.63, 153.67), "value": (97.79, 158.75)},
    "Q2": {"ref": (100.33, 140.97), "value": (116.84, 151.13)},
    "D3": {"ref": (214.63, 138.43), "value": (217.17, 144.78)},
}

# ---------------------------------------------------------------------------
# Emit.
# ---------------------------------------------------------------------------
ROOT_UUID = "e63e39d7-6ac0-4ffd-9b4c-4a61a49d5db0"
body = []

for title, gx1, gy1, gx2, gy2 in GROUPS:
    body.append(f'  (rectangle (start {g(gx1)} {g(gy1)}) (end {g(gx2)} {g(gy2)}) (stroke (width 0.1524) (type dash)) (fill (type none)) (uuid "{U()}"))')
    body.append(f'  (text "{title}" (at {g(gx1+1.27)} {g(gy1+3.81)} 0) (effects (font (size 2 2) (bold yes)) (justify left)) (uuid "{U()}"))')

for x, y, txt in NOTES:
    body.append(f'  (text "{txt}" (at {g(x)} {g(y)} 0) (effects (font (size 1.0 1.0) (italic yes)) (justify left)) (uuid "{U()}"))')

def sym_block(lib, name, ref, value, x, y, rot, mir, props):
    mir_s = f" (mirror {mir})" if mir else ""
    return (f'''  (symbol (lib_id "{lib}:{name}") (at {g(x)} {g(y)} {rot}){mir_s} (unit 1)
    (in_bom yes) (on_board yes) (dnp no)
    (uuid "{U()}")
{props}
    (instances (project "k3pwr-dongle"
      (path "/{ROOT_UUID}" (reference "{ref}") (unit 1))))
  )''')

for ref in ORDER:
    lib, name, value, x, y, rot, mir = SYMS[ref]
    tys = [y + xf(sx, sy, rot, mir)[1] for _, _, sx, sy in sym_pins[f"{lib}:{name}"]]
    top = min(min(tys), y - 2.54)
    bot = max(max(tys), y + 2.54)
    ov = OVERRIDES.get(ref, {})
    rx, ry = ov.get("ref", (x, top - 2.54))
    vx, vy = ov.get("value", (x, bot + 2.54))
    pa = 90 if rot in (90, 270) else 0   # cancels symbol rotation -> horizontal text
    fp, lcsc = FPLCSC[ref]
    props = (f'    (property "Reference" "{ref}" (at {g(rx)} {g(ry)} {pa}) (effects (font (size 1.27 1.27))))\n'
             f'    (property "Value" "{value}" (at {g(vx)} {g(vy)} {pa}) (effects (font (size 1.27 1.27))))\n'
             f'    (property "Footprint" "{fp}" (at {g(x)} {g(y)} 0) (effects (font (size 1.27 1.27)) (hide yes)))\n'
             f'    (property "LCSC" "{lcsc}" (at {g(x)} {g(y)} 0) (effects (font (size 1.27 1.27)) (hide yes)))')
    body.append(sym_block(lib, name, ref, value, x, y, rot, mir, props))

for ref, pname, x, y in POWER:
    hide_val = pname in ("PWR_FLAG", "GND")
    vy = y - 4.45 if pname == "VBUS" else y + 2.54
    val_eff = '(effects (font (size 1.27 1.27)) (hide yes))' if hide_val else '(effects (font (size 1.27 1.27)))'
    props = (f'    (property "Reference" "{ref}" (at {g(x)} {g(y)} 0) (effects (font (size 1.27 1.27)) (hide yes)))\n'
             f'    (property "Value" "{pname}" (at {g(x)} {g(vy)} 0) {val_eff})')
    body.append(sym_block("power", pname, ref, pname, x, y, 0, None, props))

for (x1, y1), (x2, y2) in WIRES:
    body.append(f'  (wire (pts (xy {g(x1)} {g(y1)}) (xy {g(x2)} {g(y2)})) (stroke (width 0) (type default)) (uuid "{U()}"))')

for x, y in JUNCTIONS:
    body.append(f'  (junction (at {g(x)} {g(y)}) (diameter 0) (color 0 0 0 0) (uuid "{U()}"))')

for net, x, y, rot in GLABELS:
    just = "right" if rot == 180 else "left"
    body.append(f'  (global_label "{net}" (shape passive) (at {g(x)} {g(y)} {rot}) (fields_autoplaced yes) (effects (font (size 1.27 1.27)) (justify {just})) (uuid "{U()}"))')

for net, x, y in LLABELS:
    body.append(f'  (label "{net}" (at {g(x)} {g(y)} 0) (fields_autoplaced yes) (effects (font (size 1.27 1.27)) (justify left bottom)) (uuid "{U()}"))')

noconnects = []
for ref in ORDER:
    lib, name, _, x, y, rot, mir = SYMS[ref]
    for num, _, sx, sy in sym_pins[f"{lib}:{name}"]:
        if (ref, num) not in used:
            dx, dy = xf(sx, sy, rot, mir)
            noconnects.append(f'  (no_connect (at {g(x+dx)} {g(y+dy)}) (uuid "{U()}"))')

sch = f'''(kicad_sch (version 20250114) (generator "k3fall-gen") (generator_version "9.0")
  (uuid "{ROOT_UUID}")
  (paper "A4")
  (title_block (title "k3pwr USB power dongle") (company "W6JY")
    (comment 1 "CH340C DTR -> PNP inverter -> RC -> 2N7002 open collector -> K3 ACC pin 8")
    (comment 2 "Fail-safe: unplugged/unpowered/port-closed = radio OFF"))
  (lib_symbols
{chr(10).join(lib_symbols)}
  )
{chr(10).join(body)}
{chr(10).join(noconnects)}
  (sheet_instances (path "/" (page "1")))
)'''
open("k3pwr-dongle.kicad_sch", "w").write(sch)
print(f"wrote k3pwr-dongle.kicad_sch: {len(ORDER)} components, {len(POWER)} power symbols, "
      f"{len(WIRES)} wire segments, {len(GLABELS)} global labels, {len(noconnects)} NCs")
