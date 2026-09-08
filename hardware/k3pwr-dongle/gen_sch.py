#!/usr/bin/env python3
"""Generate k3pwr-dongle.kicad_sch: CH340C DTR -> PNP inverter -> RC -> 2N7002
open-collector into K3 ACC pin 8 (DE-15HD), USB pigtail pads. Symbols are
extracted verbatim from the installed KiCad libraries; nets are attached as
global labels placed exactly on pin connection points; NC pins get
no_connect markers."""
import re, uuid, sys

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
    ("Device", "C_Polarized"),
    ("Device", "LED"),
    ("Transistor_BJT", "Q_PNP_BEC"),
    ("Transistor_FET", "Q_NMOS_GSD"),
    ("Connector", "DE15_Pins_HighDensity"),
    ("power", "PWR_FLAG"),
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

def U(): return str(uuid.uuid4())

# name -> net map helpers
def ch340_net(pin_name):
    return {
        "VCC": "VBUS", "GND": "GND", "V3": "V3", "UD+": "UD+", "UD-": "UD-",
        "DTR": "DTR_N", "~{DTR}": "DTR_N", "DTR#": "DTR_N",
        "R232": "GND", "TXD": "LOOP", "RXD": "LOOP",
    }.get(pin_name.strip("~{}").replace("}", ""), None) or (
        "DTR_N" if "DTR" in pin_name else
        "GND" if "R232" in pin_name else None)

placements = []  # (lib,name,ref,value,x,y, netmap: pin_number->net or None=NC)
SLOT = 0
def place(lib, name, ref, value, netfn, note=None, note_dy=11.43):
    global SLOT
    col, row = SLOT % 9, SLOT // 9
    x = 30.48 + col * 43.18
    y = 78.74 + row * 95.25
    placements.append((lib, name, ref, value, x, y, netfn, note, note_dy))
    SLOT += 1

sym_pins = {f"{lib}:{name}": pins(extract(*PIN_SOURCE[f"{lib}:{name}"])) for lib, name in LIBS}

def netfn_fixed(d):
    return lambda num, pname: d.get(num)

place("Connector_Generic", "Conn_01x05", "J1", "USB_pigtail",
      netfn_fixed({"1": "VBUS_IN", "2": "UD-", "3": "UD+", "4": "GND", "5": "GND"}),
      note="solder pads for USB-A pigtail to the Pi\\n(VBUS, D-, D+, GND, shield->GND)")
place("Interface_USB", "CH340C", "U1", "CH340C",
      lambda num, pname: ch340_net(pname),
      note="USB-serial bridge; only DTR# is used\\nTXD->RXD loopback lets the server\\nprobe that the dongle is present", note_dy=20.32)
place("Device", "R", "R1", "4.7k", netfn_fixed({"1": "DTR_N", "2": "Q1B"}),
      note="Q1 base current limit")
place("Device", "R", "R4", "10k", netfn_fixed({"1": "VBUS", "2": "Q1B"}),
      note="holds Q1 off while DTR# floats\\nduring USB enumeration")
place("Transistor_BJT", "Q_PNP_BEC", "Q1", "MMBT3906",
      lambda num, pname: {"B": "Q1B", "E": "VBUS", "C": "GATE_PRE"}.get(pname),
      note="polarity inverter: DTR# is active-low,\\nidles high -> idle/unplugged = radio OFF")
place("Device", "R", "R2", "33k", netfn_fixed({"1": "GATE_PRE", "2": "GATE"}),
      note="with C4: ~0.3s turn-on delay")
place("Device", "C_Polarized", "C4", "10u", netfn_fixed({"1": "GATE", "2": "GND"}),
      note="gate glitch filter: with R2/R3 swallows\\nDTR pulses from USB enumeration\\n(~0.3s on, ~1s release)")
place("Device", "R", "R3", "100k", netfn_fixed({"1": "GATE", "2": "GND"}),
      note="gate pulldown: guarantees OFF when\\nunpowered; ~1s release with C4")
place("Transistor_FET", "Q_NMOS_GSD", "Q2", "2N7002",
      lambda num, pname: {"G": "GATE", "S": "GND", "D": "PWR_SINK"}.get(pname),
      note="open-collector sink for ACC pin 8:\\nonly ever pulls low, never sources")
place("Device", "C", "C1", "100n", netfn_fixed({"1": "VBUS", "2": "GND"}),
      note="CH340C VCC decoupling\\n(place at pin 16)")
place("Device", "C_Polarized", "C3", "4.7u", netfn_fixed({"1": "VBUS", "2": "GND"}),
      note="VBUS bulk reservoir\\nfor pigtail inductance")
place("Device", "C", "C2", "100n", netfn_fixed({"1": "V3", "2": "GND"}),
      note="V3 internal-regulator bypass\\n(required by CH340 datasheet)")
place("Device", "R", "R5", "1k", netfn_fixed({"1": "VBUS", "2": "LED_A"}),
      note="power LED current limit (~3mA)")
place("Device", "LED", "D1", "green", netfn_fixed({"1": "GND", "2": "LED_A"}),
      note="USB power indicator")
place("Connector", "DE15_Pins_HighDensity", "J2", "K3_ACC",
      netfn_fixed({"5": "GND", "8": "ACC_PWR"}),
      note="K3 ACC: pin 8 = PWR ON (sink = on),\\npin 5 = GND, all others unused", note_dy=25.4)
place("Device", "Polyfuse", "F1", "500mA", netfn_fixed({"1": "VBUS_IN", "2": "VBUS_F"}),
      note="protects the Pi USB port from\\nboard or hood-wiring faults")
place("Device", "D_Schottky", "D2", "SS14", netfn_fixed({"2": "VBUS_F", "1": "VBUS"}),
      note="reverse-polarity guard for a\\nmiswired pigtail (~0.3V drop)")
place("Power_Protection", "USBLC6-2P6", "U2", "USBLC6-2SC6",
      lambda num, pname: {"1": "UD-", "6": "UD-", "3": "UD+", "4": "UD+", "5": "VBUS", "2": "GND"}.get(num),
      note="IEC 61000-4-2 ESD clamp\\nfor D+/D-/VBUS")
place("Device", "D_TVS", "D3", "SMAJ15A", netfn_fixed({"1": "ACC_PWR", "2": "GND"}),
      note="clamps ESD/spikes on the ACC line;\\nmount at connector, cathode to line")
place("Device", "R", "R6", "100R", netfn_fixed({"1": "PWR_SINK", "2": "ACC_PWR"}),
      note="limits fault current into ACC wiring;\\nnegligible vs the K3 pull-up")
place("power", "PWR_FLAG", "#FLG01", "PWR_FLAG", netfn_fixed({"1": "VBUS"}))
place("power", "PWR_FLAG", "#FLG02", "PWR_FLAG", netfn_fixed({"1": "GND"}))

body = []
labels = []
noconnects = []
for lib, name, ref, value, px, py, netfn, note, note_dy in placements:
    su = U()
    body.append(f'''  (symbol (lib_id "{lib}:{name}") (at {px} {py} 0) (unit 1)
    (in_bom yes) (on_board yes) (dnp no)
    (uuid "{su}")
    (property "Reference" "{ref}" (at {px} {py-7.62} 0) (effects (font (size 1.27 1.27))))
    (property "Value" "{value}" (at {px} {py+7.62} 0) (effects (font (size 1.27 1.27))))
    (instances (project "k3pwr-dongle"
      (path "/e63e39d7-6ac0-4ffd-9b4c-4a61a49d5db0" (reference "{ref}") (unit 1))))
  )''')
    if note:
        for li, linetxt in enumerate(note.split("\\n")):
            body.append(f'  (text "{linetxt}" (at {px} {py+note_dy+li*2.54} 0) (effects (font (size 1.0 1.0) (italic yes)) (justify left)) (uuid "{U()}"))')
    for num, pname, sx, sy in sym_pins[f"{lib}:{name}"]:
        ax, ay = px + sx, py - sy
        net = netfn(num, pname)
        if net:
            labels.append(f'  (global_label "{net}" (shape passive) (at {ax} {ay} 0) (fields_autoplaced yes) (effects (font (size 1.27 1.27)) (justify left)) (uuid "{U()}"))')
        else:
            noconnects.append(f'  (no_connect (at {ax} {ay}) (uuid "{U()}"))')

sch = f'''(kicad_sch (version 20250114) (generator "k3fall-gen") (generator_version "9.0")
  (uuid "e63e39d7-6ac0-4ffd-9b4c-4a61a49d5db0")
  (paper "A3")
  (title_block (title "k3pwr USB power dongle") (company "W6JY")
    (comment 1 "CH340C DTR -> PNP inverter -> RC -> 2N7002 open collector -> K3 ACC pin 8")
    (comment 2 "Fail-safe: unplugged/unpowered/port-closed = radio OFF"))
  (lib_symbols
{chr(10).join(lib_symbols)}
  )
{chr(10).join(body)}
{chr(10).join(labels)}
{chr(10).join(noconnects)}
  (sheet_instances (path "/" (page "1")))
)'''
open("k3pwr-dongle.kicad_sch", "w").write(sch)
print(f"wrote k3pwr-dongle.kicad_sch: {len(placements)} symbols, {len(labels)} labels, {len(noconnects)} NCs")
