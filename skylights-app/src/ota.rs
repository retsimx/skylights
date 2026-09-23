//! OTA trigger seam for the Skylights controller.
//!
//! This module owns the signal that `skylight/reset` raises to request an
//! over-the-air update check. SL-7 (#8) replaces the placeholder consumer with
//! the real OTA download/apply task; until then [`ota_placeholder_task`] exists
//! only to make the trigger observable (grug 9) and to avoid a dead static.

use embassy_executor::task;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use esp_println::println;

/// Raised when `skylight/reset` requests an OTA check.
///
/// SL-7 (#8) consumes this signal; until then the placeholder task logs it.
pub static OTA_TRIGGER: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Temporary consumer of [`OTA_TRIGGER`] until SL-7 (#8) lands.
///
/// Logs each request and then waits for the next one. This task is replaced
/// wholesale by the real OTA task in #8; it performs no network or flash work.
#[task]
pub async fn ota_placeholder_task() -> ! {
    loop {
        OTA_TRIGGER.wait().await;
        println!("OTA check requested (waiting on SL-7)");
    }
}
