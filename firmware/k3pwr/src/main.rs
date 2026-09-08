//! k3pwr — remote K3 power control for the k3fall panadapter.
//!
//! Waveshare ESP32-P4-Module-DEV-KIT plugged into the Pi over USB. The
//! P4's USB-Serial-JTAG console enumerates as /dev/ttyACM0 on the Pi;
//! the k3fall server writes line commands to it:
//!
//!   on\n      -> drive GPIO4 high  -> transistor sinks ACC pin 8 -> K3 ON
//!   off\n     -> drive GPIO4 low   -> line released              -> K3 OFF
//!   status\n  -> report without changing anything
//!
//! Every command is answered with "ok on" or "ok off". Level semantics
//! mirror RemoteRig OUT0 "Connect" mode, via the external open-collector
//! stage (2N7000) into the RemoteRig E3c cable's RJ45 pin 5. The GPIO
//! never sources voltage toward the radio — the transistor only sinks,
//! and the gate pulldown keeps the radio off while unpowered/booting.

use esp_idf_svc::hal::gpio::PinDriver;
use esp_idf_svc::hal::peripherals::Peripherals;
use std::io::{BufRead, Write};

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    // Power stage gate: low at boot = transistor off = radio line released.
    let mut power_pin = PinDriver::output(peripherals.pins.gpio4)?;
    power_pin.set_low()?;
    let mut on = false;

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    // Announce readiness on the wire; the server ignores anything that
    // isn't an "ok ..." ack, so banners and logs are harmless.
    let _ = writeln!(stdout, "k3pwr ready");

    let mut line = String::new();
    loop {
        line.clear();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => {
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
            Ok(_) => {}
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
        }
        match line.trim() {
            "on" => {
                power_pin.set_high()?; // sink ACC pin 8 -> radio ON
                on = true;
            }
            "off" => {
                power_pin.set_low()?; // release -> radio OFF
                on = false;
            }
            "status" => {}
            "" => continue,
            other => {
                let _ = writeln!(stdout, "err unknown command: {other}");
                continue;
            }
        }
        let _ = writeln!(stdout, "ok {}", if on { "on" } else { "off" });
    }
}
