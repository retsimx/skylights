//! Skylights firmware entry point for ESP32.

#![no_std]
#![no_main]

extern crate alloc;

pub mod flash;
pub mod heap;
pub mod mqtt;
pub mod net;
pub mod ota;
pub mod secrets;
pub mod window;

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::gpio::{Level, Output};
use esp_hal::rng::Rng;
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

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    // Initialize 96 KiB static heap allocator
    heap::init_heap();

    // Verify heap dynamic allocation
    verify_heap_allocations();

    log_boot_banner();

    // esp-wifi drives TIMG0; embassy time drives TIMG1. The timer groups are
    // never reused.
    let timg1 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG1);
    // `TimerGroup` is not `Drop`, so `wdt` and `timer0` are moved out
    // independently; the WDT guards the post-swap self-test window.
    let wdt = timg1.wdt;
    esp_hal_embassy::init(timg1.timer0);

    // `Rng` is `Copy` over the phantom `RNG` peripheral, so the same instance
    // is shared by esp-wifi here and by the OTA task below.
    let rng = Rng::new(peripherals.RNG);

    let stack = net::init(
        &spawner,
        peripherals.TIMG0,
        rng,
        peripherals.RADIO_CLK,
        peripherals.WIFI,
    );
    println!("Wi-Fi station and network tasks spawned");

    // All pins initialised Level::High (inactive) -> boot invariant.
    let w1 = window::WindowPins::new(
        Output::new(peripherals.GPIO2, Level::High),
        Output::new(peripherals.GPIO4, Level::High),
        Output::new(peripherals.GPIO16, Level::High),
    );
    let w2 = window::WindowPins::new(
        Output::new(peripherals.GPIO19, Level::High),
        Output::new(peripherals.GPIO5, Level::High),
        Output::new(peripherals.GPIO18, Level::High),
    );
    let w3 = window::WindowPins::new(
        Output::new(peripherals.GPIO21, Level::High),
        Output::new(peripherals.GPIO22, Level::High),
        Output::new(peripherals.GPIO23, Level::High),
    );

    // One-shot boot invariant check: every window output must be inactive (HIGH)
    // before the pin sets are moved into their drivers.
    let gpio_safe = ota::selftest::probe_gpio_safe(&[&w1, &w2, &w3]);
    println!(
        "GPIO boot probe: all window outputs inactive = {}",
        gpio_safe
    );

    spawner
        .spawn(window::window_task(0, window::WindowDriver::new(w1)))
        .unwrap();
    spawner
        .spawn(window::window_task(1, window::WindowDriver::new(w2)))
        .unwrap();
    spawner
        .spawn(window::window_task(2, window::WindowDriver::new(w3)))
        .unwrap();

    println!(
        "Window controller tasks spawned ({} windows)",
        window::WINDOW_COUNT
    );

    if spawner.spawn(mqtt::mqtt_task(stack)).is_err() {
        println!("ERROR: failed to spawn mqtt_task; MQTT will not run");
    }
    if spawner.spawn(ota::ota_task(stack, rng)).is_err() {
        println!("ERROR: failed to spawn ota_task; OTA updates will not run");
    }
    if spawner
        .spawn(ota::selftest::self_test_task(wdt, gpio_safe))
        .is_err()
    {
        println!("ERROR: failed to spawn self_test_task; trial boot will not be confirmed");
    }
    // One check at boot, in addition to the `skylight/reset` MQTT trigger.
    ota::OTA_TRIGGER.signal(());

    loop {
        Timer::after(Duration::from_secs(3600)).await;
    }
}
