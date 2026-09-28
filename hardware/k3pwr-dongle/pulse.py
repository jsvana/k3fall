#!/usr/bin/env python3
"""Pulse DTR on the k3pwr dongle to toggle the K3's power.

Stdlib-only (drives DTR via ioctl, no pyserial); macOS modem-control
constants.  The gate RC needs DTR held ~0.3s worst-case before Q2 turns
on, so the default 1s hold has margin; sub-0.1s holds won't trigger by
design (glitch immunity).

Usage:
    ./pulse.py                 # auto-detect port, hold DTR 1s
    ./pulse.py --hold 2.5      # longer hold
    ./pulse.py --port /dev/cu.usbserial-10
"""
import argparse
import fcntl
import glob
import os
import struct
import sys
import termios
import time

TIOCMBIS = 0x8004746C  # set modem bits
TIOCMBIC = 0x8004746B  # clear modem bits
TIOCM_DTR = 0x002


def find_port():
    ports = sorted(glob.glob("/dev/cu.usbserial*") + glob.glob("/dev/cu.wchusbserial*"))
    if not ports:
        sys.exit("no CH340 serial port found -- is the dongle plugged in?")
    if len(ports) > 1:
        sys.exit(f"multiple candidate ports, pick one with --port: {', '.join(ports)}")
    return ports[0]


def main():
    ap = argparse.ArgumentParser(description="Pulse DTR to toggle K3 power")
    ap.add_argument("--port", help="serial device (default: auto-detect)")
    ap.add_argument("--hold", type=float, default=1.0,
                    help="seconds to hold DTR asserted (default 1.0)")
    args = ap.parse_args()

    port = args.port or find_port()
    try:
        fd = os.open(port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    except OSError as e:
        sys.exit(f"cannot open {port}: {e.strerror} (port in use by another program?)")

    attrs = termios.tcgetattr(fd)
    attrs[2] |= termios.CLOCAL
    termios.tcsetattr(fd, termios.TCSANOW, attrs)

    dtr = struct.pack("I", TIOCM_DTR)
    fcntl.ioctl(fd, TIOCMBIC, dtr)   # known-clear starting state
    time.sleep(0.05)
    fcntl.ioctl(fd, TIOCMBIS, dtr)
    print(f"{port}: DTR asserted for {args.hold:g}s")
    time.sleep(args.hold)
    fcntl.ioctl(fd, TIOCMBIC, dtr)
    time.sleep(0.1)
    os.close(fd)
    print("DTR released")


if __name__ == "__main__":
    main()
