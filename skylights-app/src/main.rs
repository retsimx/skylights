//! Skylights firmware entry point for ESP32.

#![no_std]
#![no_main]

extern crate alloc;

pub mod flash;
pub mod heap;
pub mod secrets;

use esp_backtrace as _;
use esp_hal::main;
use esp_println::println;

/// Verifies basic dynamic allocation functionality from the static heap.
fn verify_heap_allocations() {
    use alloc::boxed::Box;
    use alloc::vec;

    let boxed_val = Box::new(42u32);
    assert_eq!(*boxed_val, 42);

    let mut test_vec = vec![10u8, 20u8];
    test_vec.push(30u8);
    assert_eq!(test_vec.len(), 3);
    assert_eq!(test_vec[2], 30);
}

/// Logs the startup banner including build version, git commit hash, and active flash slot.
fn log_boot_banner() {
    let build_version = env!("SKYLIGHTS_BUILD_VERSION");
    let git_hash = env!("SKYLIGHTS_GIT_HASH");

    let flash_storage = flash::EspFlashStorage::new();
    match flash_storage.resolve_otadata() {
        Ok(res) => {
            println!(
                "Booting Skylights firmware v{} ({}) - Active slot: {} (seq: {}, trial: {})",
                build_version,
                git_hash,
                res.active_slot.name(),
                res.active_seq,
                res.is_trial
            );
        }
        Err(err) => {
            println!(
                "Booting Skylights firmware v{} ({}) - Failed to resolve boot slot: {}. Defaulting to ota_0",
                build_version,
                git_hash,
                err
            );
        }
    }
}

#[main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    // Initialize 48 KiB static heap allocator
    heap::init_heap();

    // Verify heap dynamic allocation
    verify_heap_allocations();

    // Basic sanity verification of core crate integration
    let _initial_position = skylights_core::Position::CLOSED;
    let _initial_state = skylights_core::WindowState::Closed;

    log_boot_banner();

    #[allow(clippy::empty_loop)]
    loop {}
}
