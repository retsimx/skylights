//! Skylights firmware entry point for ESP32.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::main;
use esp_println as _;

#[main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    // Basic sanity verification of core crate integration
    let _initial_position = skylights_core::Position::CLOSED;
    let _initial_state = skylights_core::WindowState::Closed;

    #[allow(clippy::empty_loop)]
    loop {}
}
