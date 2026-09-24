//! Static heap allocator for dynamic memory allocations.
//!
//! Uses [`esp_alloc::HEAP`] as the `#[global_allocator]` and registers one
//! 96 KiB backing region at boot. This is the canonical Espressif allocator:
//! `esp-wifi` routes its internal C `malloc`/`free` through `esp_alloc::HEAP`
//! automatically, so no hand-written allocator FFI shims are required.
//!
//! The region lives in **DRAM2** (`.dram2_uninit`), not DRAM0. DRAM0 must hold
//! the main stack — which is the space left after every static section — next
//! to the OTA/MQTT buffers and the task arena; a 96 KiB heap in DRAM0 starves
//! the stack (observed as a Wi-Fi `ppTask` fault). DRAM2 (~96.4 KiB) is
//! otherwise unusable on the ESP32 and is the canonical home for this heap.

use core::mem::MaybeUninit;
use core::ptr::addr_of_mut;

/// Total heap size in bytes (96 KiB = 98,304 bytes).
///
/// Sized for `esp-wifi` radio buffers plus `embassy-net` DHCPv4, DNS and socket
/// bookkeeping, with headroom for the TLS allocations of a streaming HTTPS OTA.
pub const HEAP_SIZE: usize = 96 * 1024;

/// Initializes the 96 KiB static heap allocator in DRAM2.
///
/// Must be invoked early during boot before any dynamic allocation. Registers
/// the region with the global [`esp_alloc::HEAP`] and must only be called once.
pub fn init_heap() {
    #[link_section = ".dram2_uninit"]
    static mut HEAP_MEM: MaybeUninit<[u8; HEAP_SIZE]> = MaybeUninit::uninit();

    // SAFETY: `HEAP_MEM` is a `'static`, exclusively-owned, never-aliased region
    // of exactly `HEAP_SIZE` bytes; `init_heap` is called once at boot.
    unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
            addr_of_mut!(HEAP_MEM).cast::<u8>(),
            HEAP_SIZE,
            esp_alloc::MemoryCapability::Internal.into(),
        ));
    }
}
