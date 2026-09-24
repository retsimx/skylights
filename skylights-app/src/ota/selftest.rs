//! Post-swap self-test task and boot confirmation for the Skylights controller.
//!
//! After an OTA trial boot the new image must prove itself within a bounded
//! window before the bootloader keeps it. This module glues the pure
//! [`skylights_core::ota::SelfTestTracker`] to hardware: it reads `otadata`,
//! probes the window outputs at boot, observes Wi-Fi and MQTT health, feeds an
//! 8-second hardware watchdog while the 30-second window elapses, and either
//! confirms the running slot (`ESP_OTA_IMG_VALID`) or resets for bootloader
//! rollback.
//!
//! A normal boot whose `otadata` already reads valid returns immediately: no
//! watchdog is armed and no delay is added.

use embassy_time::{Duration, Instant, Timer};
use esp_hal::reset::software_reset;
use esp_hal::time::ExtU64;
use esp_hal::timer::timg::{MwdtStage, Wdt};
use esp_println::println;
use skylights_core::ota::{Clock, SelfTestTracker, SelfTestVerdict};
use skylights_core::{FlashError, LinkState, OtaStorage};

use crate::flash::EspFlashStorage;

/// Hardware watchdog timeout guarding the trial window, in microseconds.
const WATCHDOG_TIMEOUT_US: u64 = 8_000_000;

/// Self-test poll period, in milliseconds.
const POLL_PERIOD_MS: u64 = 100;

/// Minimum interval between watchdog feeds, in milliseconds.
const FEED_INTERVAL_MS: u64 = 1_000;

/// Monotonic embassy-time clock adapter for the core self-test tracker.
struct InstantClock {
    start: Instant,
}

impl Clock for InstantClock {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis()
    }
}

/// One-shot boot probe: all nine window outputs inactive (HIGH).
pub fn probe_gpio_safe(pins: &[&crate::window::WindowPins<'_>]) -> bool {
    pins.iter().all(|p| p.all_inactive())
}

/// Confirms the running slot as permanently valid (`ESP_OTA_IMG_VALID`).
async fn mark_booted(storage: &mut EspFlashStorage) -> Result<(), FlashError> {
    storage.mark_valid().await
}

/// Runs the post-swap self-test, confirming or rolling back the trial image.
///
/// Returns without arming the watchdog when `otadata` is unreadable or the
/// active slot is already valid. Otherwise arms `wdt` for the trial window,
/// polls the health signals every [`POLL_PERIOD_MS`], and feeds the watchdog at
/// most once per [`FEED_INTERVAL_MS`]. A confirmed slot disables the watchdog
/// and returns; a failed verdict logs and resets for rollback.
#[embassy_executor::task]
pub async fn self_test_task(mut wdt: Wdt<esp_hal::peripherals::TIMG1>, gpio_safe: bool) {
    let mut storage = EspFlashStorage::new();

    match storage.resolve_otadata() {
        Ok(res) if !res.is_trial => {
            println!("SELF-TEST: skipped (slot already valid)");
            return;
        }
        Ok(_) => {}
        Err(_) => {
            println!("SELF-TEST: otadata read failed, skipping confirmation");
            return;
        }
    }

    wdt.set_timeout(MwdtStage::Stage0, WATCHDOG_TIMEOUT_US.micros());
    wdt.enable();
    wdt.feed();

    let Some(mut wifi) = crate::net::WIFI_CONNECTED.receiver() else {
        println!("SELF-TEST: WIFI_CONNECTED has no free receiver slot, resetting");
        software_reset();
        return;
    };
    let Some(mut mqtt) = crate::mqtt::MQTT_HEALTHY.receiver() else {
        println!("SELF-TEST: MQTT_HEALTHY has no free receiver slot, resetting");
        software_reset();
        return;
    };

    let start = Instant::now();
    let clock = InstantClock { start };
    let mut tracker = SelfTestTracker::new(0, gpio_safe);
    let mut last_feed_ms: u64 = 0;

    loop {
        let now = clock.now_ms();
        let wifi_up = wifi.try_get() == Some(LinkState::Connected);
        let mqtt_up = mqtt.try_get().unwrap_or(false);
        tracker.observe(wifi_up, mqtt_up);

        match tracker.evaluate(&clock) {
            SelfTestVerdict::Pending => {}
            SelfTestVerdict::Passed => {
                if let Err(e) = mark_booted(&mut storage).await {
                    println!("SELF-TEST: mark_booted failed {:?}", e);
                    software_reset();
                    return;
                }
                wdt.disable();
                println!("SELF-TEST: passed, slot confirmed");
                return;
            }
            SelfTestVerdict::Failed(reason) => {
                println!("SELF-TEST: failed reason={}", reason);
                software_reset();
                return;
            }
        }

        if now.saturating_sub(last_feed_ms) >= FEED_INTERVAL_MS {
            wdt.feed();
            last_feed_ms = now;
        }
        Timer::after(Duration::from_millis(POLL_PERIOD_MS)).await;
    }
}
