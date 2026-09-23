//! Static heap allocator for dynamic memory allocations.
//!
//! Provides a 48 KiB static buffer managed by [`embedded_alloc::LlffHeap`]
//! as the `#[global_allocator]`. This satisfies dynamic memory requirements
//! for cryptographic routines (such as TLS certificate verification in `embedded-tls`)
//! in a `no_std` bare-metal environment without depending on an underlying OS heap.

use core::mem::MaybeUninit;

#[global_allocator]
static HEAP: embedded_alloc::LlffHeap = embedded_alloc::LlffHeap::empty();

/// Total heap size in bytes (48 KiB = 49,152 bytes).
pub const HEAP_SIZE: usize = 48 * 1024;

/// Initializes the 48 KiB static heap allocator.
///
/// Must be invoked early during boot before any dynamic allocations (e.g. `alloc::vec::Vec`
/// or `alloc::boxed::Box`) are attempted.
pub fn init_heap() {
    static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
    unsafe {
        // Use addr_of_mut to safely obtain raw pointer without creating a mutable reference,
        // avoiding static_mut_refs compiler warnings/errors.
        let ptr = core::ptr::addr_of_mut!(HEAP_MEM) as *mut u8;
        HEAP.init(ptr as usize, HEAP_SIZE);
    }
}
