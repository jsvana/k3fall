//! k3pwr — remote K3 power control for the k3fall panadapter.
//!
//! Waveshare ESP32-P4-Module-DEV-KIT, wired Ethernet (IP101GRI on RMII,
//! brought up by the espressif/ethernet_init component from sdkconfig).
//! GPIO4 drives an external open-collector stage (2N7000) that pulls the
//! K3 ACC "PWR ON" line (RemoteRig E3c cable, RJ45 pin 5) to ground.
//! Level semantics, mirroring RemoteRig OUT0 "Connect" mode: stage
//! conducting = radio on, released = radio off. The GPIO never sources
//! voltage toward the radio — the transistor only sinks.
//!
//! HTTP API (port 80):
//!   GET  /power  -> {"on":bool}
//!   POST /power  {"on":true|false} -> {"on":bool}
//!   GET  /health -> ok

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio::{Output, PinDriver};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::http::server::{Configuration as HttpConfig, EspHttpServer};
use esp_idf_svc::http::Method;
use esp_idf_svc::io::{Read, Write};
use esp_idf_svc::mdns::EspMdns;
use esp_idf_svc::netif::{EspNetif, NetifConfiguration, NetifStack};
use esp_idf_svc::sys::{
    esp, esp_eth_handle_t, esp_eth_new_netif_glue, esp_eth_start, esp_netif_attach,
    ethernet_init_all,
};

static POWER_ON: AtomicBool = AtomicBool::new(false);

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let _sys_loop = EspSystemEventLoop::take()?; // default loop for netif/eth events

    // Power stage gate: low at boot = transistor off = radio line released.
    let mut power_pin = PinDriver::output(peripherals.pins.gpio4)?;
    power_pin.set_low()?;
    let power_pin: Arc<Mutex<PinDriver<'static, _, Output>>> = Arc::new(Mutex::new(power_pin));
    log::info!("power stage on GPIO4, boot state OFF");

    // Ethernet via the espressif/ethernet_init component (pins/PHY from
    // sdkconfig.defaults: IP101, MDC=31, MDIO=52, RST=51, addr 1).
    let mut eth_handles: *mut esp_eth_handle_t = std::ptr::null_mut();
    let mut eth_count: u8 = 0;
    esp!(unsafe { ethernet_init_all(&mut eth_handles, &mut eth_count) })?;
    anyhow::ensure!(eth_count > 0, "ethernet_init_all found no interfaces");
    let eth_handle = unsafe { *eth_handles };
    log::info!("ethernet_init_all: {eth_count} interface(s)");

    let netif = EspNetif::new_with_conf(&NetifConfiguration::eth_default_client())?;
    let glue = unsafe { esp_eth_new_netif_glue(eth_handle) };
    anyhow::ensure!(!glue.is_null(), "esp_eth_new_netif_glue failed");
    esp!(unsafe { esp_netif_attach(netif.handle(), glue as *mut _) })?;
    esp!(unsafe { esp_eth_start(eth_handle) })?;
    log::info!("ethernet started, waiting for DHCP...");

    // Poll for an address rather than wiring event handlers; power control
    // has no hurry and this keeps the code small.
    let ip = loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let info = netif.get_ip_info()?;
        if info.ip.is_unspecified() {
            continue;
        }
        break info;
    };
    log::info!("ethernet up: {:?}", ip);

    // mDNS: reachable as k3pwr.local
    let mut mdns = EspMdns::take()?;
    mdns.set_hostname("k3pwr")?;
    mdns.add_service(None, "_http", "_tcp", 80, &[])?;

    let mut server = EspHttpServer::new(&HttpConfig::default())?;

    server.fn_handler("/health", Method::Get, |req| {
        req.into_ok_response()?.write_all(b"ok")
    })?;

    server.fn_handler("/power", Method::Get, |req| {
        let body = format!("{{\"on\":{}}}", POWER_ON.load(Ordering::SeqCst));
        req.into_response(200, Some("OK"), &[("Content-Type", "application/json")])?
            .write_all(body.as_bytes())
    })?;

    {
        let power_pin = power_pin.clone();
        server.fn_handler::<anyhow::Error, _>("/power", Method::Post, move |mut req| {
            let mut buf = [0u8; 128];
            let len = req.read(&mut buf)?;
            let parsed: serde_json::Value = serde_json::from_slice(&buf[..len])?;
            let Some(on) = parsed.get("on").and_then(|v| v.as_bool()) else {
                req.into_status_response(400)?
                    .write_all(b"{\"error\":\"expected {\\\"on\\\":bool}\"}")?;
                return Ok(());
            };
            {
                let mut pin = power_pin.lock().unwrap();
                if on {
                    pin.set_high()?; // transistor sinks ACC pin 8 -> radio ON
                } else {
                    pin.set_low()?; // release -> radio OFF
                }
            }
            POWER_ON.store(on, Ordering::SeqCst);
            log::info!("power set: {}", if on { "ON" } else { "OFF" });
            let body = format!("{{\"on\":{on}}}");
            req.into_response(200, Some("OK"), &[("Content-Type", "application/json")])?
                .write_all(body.as_bytes())?;
            Ok(())
        })?;
    }

    log::info!("k3pwr ready at http://k3pwr.local/power");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
