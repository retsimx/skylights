//! Skylights firmware entry point for ESP32.

#![no_std]
#![no_main]

pub mod flash;

use esp_backtrace as _;
use esp_hal::main;
use esp_println::println;

#[main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    // Basic sanity verification of core crate integration
    let _initial_position = skylights_core::Position::CLOSED;
    let _initial_state = skylights_core::WindowState::Closed;

    let flash_storage = flash::EspFlashStorage::new();
    match flash_storage.resolve_otadata() {
        Ok(res) => {
            println!(
                "Booting Skylights firmware - Active slot: {} (seq: {}, trial: {})",
                res.active_slot.name(),
                res.active_seq,
                res.is_trial
            );
        }
        Err(err) => {
            println!(
                "Booting Skylights firmware - Failed to resolve boot slot: {}. Defaulting to ota_0",
                err
            );
        }
    }

    #[allow(clippy::empty_loop)]
    loop {}
}
