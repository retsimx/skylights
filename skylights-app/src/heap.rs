//! Static heap allocator for dynamic memory allocations.
//!
//! Uses [`esp_alloc::HEAP`] as the `#[global_allocator]`. The
//! [`esp_alloc::heap_allocator!`] macro registers one static backing region with
//! that allocator at boot. This is the canonical Espressif allocator:
//! `esp-wifi` routes its internal C `malloc`/`free` through `esp_alloc::HEAP`
//! automatically, so no hand-written allocator FFI shims are required.
//!
//! The backing region is aligned internally by the allocator, so no manual
//! alignment wrapper is needed.

/// Total heap size in bytes (96 KiB = 98,304 bytes).
///
/// The previous 48 KiB region was sized only for TLS. Bringing up `esp-wifi`
/// adds radio buffers plus `embassy-net` DHCPv4, DNS, and socket bookkeeping;
/// a future HTTPS OTA (SL-7) adds TLS on top, so 96 KiB leaves deliberate
/// headroom. The value is tunable at the bench step.
pub const HEAP_SIZE: usize = 96 * 1024;

/// Initializes the 96 KiB static heap allocator.
///
/// Must be invoked early during boot before any dynamic allocations (e.g. `alloc::vec::Vec`
/// or `alloc::boxed::Box`) are attempted. The macro registers the region with the global
/// [`esp_alloc::HEAP`] and must only be invoked once per program.
pub fn init_heap() {
    esp_alloc::heap_allocator!(HEAP_SIZE);
}
