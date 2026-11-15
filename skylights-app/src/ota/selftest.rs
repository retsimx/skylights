//! Post-swap self-test, boot confirmation, and the runtime hardware watchdog.
//!
//! One task owns both because they share the only TIMG1 watchdog instance.
//!
//! 1. **Trial window.** After an OTA trial boot the new image must prove itself
//!    within a bounded window before the bootloader keeps it. This module glues
//!    the pure [`skylights_core::ota::SelfTestTracker`] to hardware: it reads
//!    `otadata`, probes the window outputs at boot, observes Wi-Fi and MQTT
//!    health, feeds the watchdog while the 30-second window elapses, and either
//!    confirms the running slot (`ESP_OTA_IMG_VALID`) or resets for bootloader
//!    rollback.
//! 2. **Runtime supervision.** Every boot — trial or not — ends in
//!    [`runtime_watchdog`], which never returns and keeps the same watchdog
//!    armed for the life of the firmware.
//!
//! The runtime half exists because every task-level recovery in this firmware
//! needs the executor to still schedule that task. A wedged executor, a stuck
//! driver call or a poisoned allocator disables all of them at once, which is
//! the "alive but unreachable" state nothing else can end. So the watchdog is
//! fed only while the MQTT path is provably alive (a completed dispatch inside
//! `NO_IP_REBOOT_MS`, or a boot still inside that grace); when it stops being
//! fed the hardware resets the chip.

use embassy_time::{Duration, Instant, Timer};
use esp_hal::reset::software_reset;
use esp_hal::time::ExtU64;
use esp_hal::timer::timg::{MwdtStage, Wdt};
use esp_println::println;
use skylights_core::ota::{Clock, SelfTestTracker, SelfTestVerdict};
use skylights_core::wifi::{progress_fresh, NO_IP_REBOOT_MS};
use skylights_core::{FlashError, LinkState, OtaStorage};

use crate::flash::EspFlashStorage;

/// Hardware watchdog timeout, in microseconds.
///
/// Shared by the trial window and runtime supervision: the same instance is
/// re-armed, so a single number describes how long any wedged state survives.
const WATCHDOG_TIMEOUT_US: u64 = 8_000_000;

/// Self-test poll period, in milliseconds.
const POLL_PERIOD_MS: u64 = 100;

/// Minimum interval between watchdog feeds, in milliseconds.
///
/// Eight of these fit inside one [`WATCHDOG_TIMEOUT_US`] window, and a feed is
/// the only thing that can extend the countdown. A wedged state is therefore
/// reset within one timeout plus at most one interval, and neither loop in this
/// module can keep the chip alive by feeding faster than the timeout counts
/// down.
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

/// Arms the TIMG1 watchdog with the firmware's one timeout and feeds it once.
///
/// [`Wdt::enable`] already programs stage 0 as a full system reset and leaves
/// stages 1-3 off, so an expiry reboots the chip with no extra setup here — and
/// an explicit stage action set before `enable()` is overwritten by it anyway.
fn arm_watchdog(wdt: &mut Wdt<esp_hal::peripherals::TIMG1>) {
    wdt.set_timeout(MwdtStage::Stage0, WATCHDOG_TIMEOUT_US.micros());
    wdt.enable();
    wdt.feed();
}

/// Runs the post-swap self-test, confirming or rolling back the trial image,
/// then arms the runtime watchdog for the rest of the boot.
///
/// A trial image arms `wdt` for the trial window, polls the health signals
/// every [`POLL_PERIOD_MS`], and feeds the watchdog at most once per
/// [`FEED_INTERVAL_MS`]. A confirmed slot falls through into
/// [`runtime_watchdog`]; a failed verdict logs and resets for rollback. A normal
/// boot (slot already valid) and an unreadable `otadata` skip the trial window
/// and go straight to runtime supervision — no boot path leaves the chip
/// without a watchdog.
#[embassy_executor::task]
pub async fn self_test_task(mut wdt: Wdt<esp_hal::peripherals::TIMG1>, gpio_safe: bool) {
    let mut storage = EspFlashStorage::new();
    // Reference point for the runtime watchdog's boot grace: a fresh boot that
    // has not dispatched yet is not a wedged one.
    let boot = Instant::now();

    match storage.resolve_otadata() {
        Ok(res) if !res.is_trial => {
            println!("SELF-TEST: skipped (slot already valid)");
            runtime_watchdog(wdt, boot).await;
        }
        Ok(_) => {}
        Err(_) => {
            // Nothing to confirm, but the chip still needs a supervisor: an
            // unreadable otadata is exactly when no other recovery exists.
            println!("SELF-TEST: otadata read failed, skipping confirmation");
            runtime_watchdog(wdt, boot).await;
        }
    }

    arm_watchdog(&mut wdt);

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
                println!("SELF-TEST: passed, slot confirmed");
                runtime_watchdog(wdt, boot).await;
            }
            SelfTestVerdict::Failed(reason) => {
                println!("SELF-TEST: failed reason={}", reason);
                // Force a rollback: mark the running slot INVALID so the
                // bootloader boots the other slot, independent of its
                // APP_ROLLBACK configuration.
                if let Err(e) = storage.mark_invalid().await {
                    println!("SELF-TEST: mark_invalid failed {:?}", e);
                }
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

/// Keeps the hardware watchdog armed forever, feeding it only while the MQTT
/// path is provably alive. Never returns.
///
/// Withholding a feed for [`WATCHDOG_TIMEOUT_US`] resets the chip. That is the
/// only recovery that survives a wedged executor, so this loop must not stop
/// for any reason: it can log, it can skip a feed, it cannot `break`.
///
/// Health is the same end-to-end signal the network supervisor resets on: a
/// completed MQTT dispatch. Before the first dispatch the boot grace applies, so
/// a slow first association is not a reset.
///
/// `MQTT_PROGRESS` is read directly rather than through a `Watch` receiver, so
/// this loop holds no receiver slot and cannot be quietly starved of one.
async fn runtime_watchdog(mut wdt: Wdt<esp_hal::peripherals::TIMG1>, boot: Instant) -> ! {
    arm_watchdog(&mut wdt);
    println!(
        "SELF-TEST: runtime watchdog armed ({}s, fed only while MQTT progresses)",
        WATCHDOG_TIMEOUT_US / 1_000_000
    );

    let mut healthy = true;
    loop {
        Timer::after(Duration::from_millis(FEED_INTERVAL_MS)).await;

        // At most one feed per FEED_INTERVAL_MS: the loop period *is* the
        // interval, so no wedged state can be held alive by feeding faster
        // than the timeout counts down.
        let now_healthy = mqtt_alive(boot);
        if now_healthy {
            wdt.feed();
            if !healthy {
                println!("SELF-TEST: MQTT progress recovered; feeding the watchdog again");
            }
        } else if healthy {
            println!(
                "SELF-TEST: no MQTT progress for {}s; withholding watchdog feeds so the chip resets",
                NO_IP_REBOOT_MS / 1000
            );
        }
        healthy = now_healthy;
    }
}

/// True while the MQTT path is alive: a completed dispatch inside
/// [`NO_IP_REBOOT_MS`], or a boot that has never dispatched and is still inside
/// that grace.
fn mqtt_alive(boot: Instant) -> bool {
    progress_fresh(crate::net::progress_age(boot.elapsed()).as_millis())
}
