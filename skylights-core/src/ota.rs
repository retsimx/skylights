//! Over-The-Air (OTA) update domain models, partition table geometry,
//! IEEE 802.3 CRC-32, bootloader otadata serialization, and storage abstraction.

use core::fmt;

/// Maximum binary image size supported by symmetric OTA slots (1856 KiB = 1,900,544 bytes).
pub const OTA_SLOT_SIZE: u32 = 1856 * 1024;

/// Flash byte offset for the primary application slot (`ota_0`).
pub const OTA_0_OFFSET: u32 = 0x020000;

/// Flash byte offset for the secondary application slot (`ota_1`).
pub const OTA_1_OFFSET: u32 = 0x1F0000;

/// Flash byte offset for the ESP32 `otadata` partition.
pub const OTADATA_OFFSET: u32 = 0x00E000;

/// Total size of the `otadata` partition (8 KiB = 2 sectors of 4 KiB each).
pub const OTADATA_SIZE: u32 = 0x002000;

/// Size of one flash sector (4 KiB = 4096 bytes).
pub const FLASH_SECTOR_SIZE: u32 = 4096;

/// New image state (flashed, not yet booted).
pub const ESP_OTA_IMG_NEW: u32 = 0;

/// Trial boot state (booted once in test mode, pending health verification).
pub const ESP_OTA_IMG_PENDING_VERIFY: u32 = 1;

/// Confirmed operational image (rollback cancelled, permanent active slot).
pub const ESP_OTA_IMG_VALID: u32 = 2;

/// Faulty image marked invalid (bootloader will automatically roll back).
pub const ESP_OTA_IMG_INVALID: u32 = 3;

/// Aborted update state.
pub const ESP_OTA_IMG_ABORTED: u32 = 4;

/// Undefined / erased flash state (0xFFFFFFFF).
pub const ESP_OTA_IMG_UNDEFINED: u32 = 0xFFFF_FFFF;

/// Dual application slots for OTA updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// Slot 0 (`ota_0`), starting at offset `0x020000`.
    Ota0,
    /// Slot 1 (`ota_1`), starting at offset `0x1F0000`.
    Ota1,
}

impl Slot {
    /// Returns the other (alternate / passive) slot.
    #[inline]
    pub const fn other(&self) -> Self {
        match self {
            Self::Ota0 => Self::Ota1,
            Self::Ota1 => Self::Ota0,
        }
    }

    /// Returns the absolute flash byte offset of this slot.
    #[inline]
    pub const fn offset(&self) -> u32 {
        match self {
            Self::Ota0 => OTA_0_OFFSET,
            Self::Ota1 => OTA_1_OFFSET,
        }
    }

    /// Returns the maximum allowable binary size for this slot (1856 KiB).
    #[inline]
    pub const fn size(&self) -> u32 {
        OTA_SLOT_SIZE
    }

    /// Returns the partition name string.
    #[inline]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Ota0 => "ota_0",
            Self::Ota1 => "ota_1",
        }
    }

    /// Resolves slot assignment from an otadata sequence number.
    ///
    /// Sequence rules:
    /// - `seq == 0` -> Ota0
    /// - `(seq - 1) % 2 == 0` -> Ota0 (seq 1, 3, 5, ...)
    /// - `(seq - 1) % 2 != 0` -> Ota1 (seq 2, 4, 6, ...)
    #[inline]
    pub const fn from_seq(seq: u32) -> Self {
        if seq == 0 || (seq - 1).is_multiple_of(2) {
            Self::Ota0
        } else {
            Self::Ota1
        }
    }
}

/// Computes lookup table for IEEE 802.3 CRC-32 (polynomial `0xEDB88320`).
const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

/// Precomputed 256-entry IEEE 802.3 CRC-32 table in read-only storage.
pub const CRC32_TABLE: [u32; 256] = make_crc32_table();

/// Calculates standard IEEE 802.3 CRC-32 over a byte slice.
///
/// Reflected polynomial: `0xEDB88320`, initial CRC: `0xFFFFFFFF`, final XOR: `0xFFFFFFFF`.
/// Test vector: ASCII `"123456789"` produces `0xCBF43926`.
pub fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF;
    let mut i = 0;
    while i < data.len() {
        let byte = data[i];
        let idx = ((crc ^ (byte as u32)) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[idx];
        i += 1;
    }
    crc ^ 0xFFFF_FFFF
}

/// ESP32 2-stage bootloader `esp_ota_select_entry` descriptor structure.
///
/// Must match exact 32-byte C layout expected in `otadata` partition sectors:
/// - Offset 0..4:   `ota_seq` (little-endian u32)
/// - Offset 4..24:  `seq_label` (20 bytes padding / label)
/// - Offset 24..28: `ota_state` (little-endian u32)
/// - Offset 28..32: `crc` (little-endian CRC-32 over the 4-byte `ota_seq`)
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EspOtaSelectEntry {
    /// Monotonically increasing OTA sequence number.
    pub ota_seq: u32,
    /// Optional sequence label / zero padding.
    pub seq_label: [u8; 20],
    /// Bootloader state flag (`ESP_OTA_IMG_*`).
    pub ota_state: u32,
    /// IEEE 802.3 CRC-32 over the 4-byte `ota_seq` field (seed 0, final inversion,
    /// matching ESP-IDF `bootloader_common_ota_select_crc`).
    pub crc: u32,
}

const _: () = assert!(core::mem::size_of::<EspOtaSelectEntry>() == 32);
const _: () = assert!(core::mem::align_of::<EspOtaSelectEntry>() == 4);

impl EspOtaSelectEntry {
    /// Computes the CRC-32 over the 4-byte little-endian `ota_seq` field according to
    /// ESP-IDF `bootloader_common_ota_select_crc` rules (seed 0, reflected IEEE 802.3, final XOR 0xFFFFFFFF).
    pub fn compute_crc(&self) -> u32 {
        let bytes = self.ota_seq.to_le_bytes();
        let mut crc = 0u32;
        for &byte in &bytes {
            let idx = ((crc ^ (byte as u32)) & 0xFF) as usize;
            crc = (crc >> 8) ^ CRC32_TABLE[idx];
        }
        crc ^ 0xFFFF_FFFF
    }

    /// Verifies if the stored CRC matches the calculated CRC over `ota_seq`.
    pub fn is_crc_valid(&self) -> bool {
        self.crc == self.compute_crc()
    }

    /// Serializes the 32-byte entry to a byte buffer in little-endian format.
    pub fn to_bytes(&self) -> [u8; 32] {
        let mut buf = [0u8; 32];
        buf[0..4].copy_from_slice(&self.ota_seq.to_le_bytes());
        buf[4..24].copy_from_slice(&self.seq_label);
        buf[24..28].copy_from_slice(&self.ota_state.to_le_bytes());
        buf[28..32].copy_from_slice(&self.crc.to_le_bytes());
        buf
    }

    /// Deserializes a 32-byte entry from a byte buffer in little-endian format.
    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        let ota_seq = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut seq_label = [0u8; 20];
        seq_label.copy_from_slice(&bytes[4..24]);
        let ota_state = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let crc = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        Self {
            ota_seq,
            seq_label,
            ota_state,
            crc,
        }
    }

    /// Constructs a new entry with `ESP_OTA_IMG_NEW` state and computed CRC.
    pub fn new_trial(ota_seq: u32) -> Self {
        let mut entry = Self {
            ota_seq,
            seq_label: [0u8; 20],
            ota_state: ESP_OTA_IMG_NEW,
            crc: 0,
        };
        entry.crc = entry.compute_crc();
        entry
    }

    /// Constructs a new entry with `ESP_OTA_IMG_VALID` state and computed CRC.
    pub fn new_valid(ota_seq: u32) -> Self {
        let mut entry = Self {
            ota_seq,
            seq_label: [0u8; 20],
            ota_state: ESP_OTA_IMG_VALID,
            crc: 0,
        };
        entry.crc = entry.compute_crc();
        entry
    }

    /// Returns true if this entry has a valid CRC and is not in an invalid or aborted state.
    pub fn is_usable(&self) -> bool {
        self.is_crc_valid()
            && self.ota_seq != 0
            && self.ota_seq != 0xFFFF_FFFF
            && self.ota_state != ESP_OTA_IMG_INVALID
            && self.ota_state != ESP_OTA_IMG_ABORTED
    }
}

/// Identifies one of the two 4 KiB sectors inside the `otadata` partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtadataSector {
    /// Sector 0 at partition offset 0 (`0x00E000` in flash).
    Sector0,
    /// Sector 1 at partition offset 4096 (`0x00F000` in flash).
    Sector1,
}

impl OtadataSector {
    /// Returns the byte offset within the `otadata` partition.
    #[inline]
    pub const fn offset_in_partition(&self) -> u32 {
        match self {
            Self::Sector0 => 0,
            Self::Sector1 => FLASH_SECTOR_SIZE,
        }
    }

    /// Returns the absolute flash address of the sector.
    #[inline]
    pub const fn flash_offset(&self) -> u32 {
        OTADATA_OFFSET + self.offset_in_partition()
    }

    /// Returns the opposite sector.
    #[inline]
    pub const fn other(&self) -> Self {
        match self {
            Self::Sector0 => Self::Sector1,
            Self::Sector1 => Self::Sector0,
        }
    }
}

/// High-level resolution result from evaluating both `otadata` sectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OtadataResolution {
    /// Resolved active boot slot.
    pub active_slot: Slot,
    /// Active sequence number.
    pub active_seq: u32,
    /// Sector hosting the active entry.
    pub active_sector: OtadataSector,
    /// True if active entry is pending post-boot health verification.
    pub is_trial: bool,
}

/// Resolves the active boot slot from the two `otadata` sector entries according to
/// ESP32 second-stage bootloader arbitration rules:
/// 1. Verifies CRC-32 for Sector 0 and Sector 1.
/// 2. Discards entries with invalid CRC or state == `INVALID` / `ABORTED`.
/// 3. If both entries are usable, the one with the higher sequence number wins.
/// 4. If only one entry is usable, it wins.
/// 5. If neither entry is usable (fresh/erased flash), defaults to `Slot::Ota0` with sequence 1.
pub fn resolve_active_slot(
    entry0: &EspOtaSelectEntry,
    entry1: &EspOtaSelectEntry,
) -> OtadataResolution {
    let usable0 = entry0.is_usable();
    let usable1 = entry1.is_usable();

    let is_trial_state =
        |state: u32| state == ESP_OTA_IMG_NEW || state == ESP_OTA_IMG_PENDING_VERIFY;

    match (usable0, usable1) {
        (true, true) => {
            if entry0.ota_seq >= entry1.ota_seq {
                OtadataResolution {
                    active_slot: Slot::from_seq(entry0.ota_seq),
                    active_seq: entry0.ota_seq,
                    active_sector: OtadataSector::Sector0,
                    is_trial: is_trial_state(entry0.ota_state),
                }
            } else {
                OtadataResolution {
                    active_slot: Slot::from_seq(entry1.ota_seq),
                    active_seq: entry1.ota_seq,
                    active_sector: OtadataSector::Sector1,
                    is_trial: is_trial_state(entry1.ota_state),
                }
            }
        }
        (true, false) => OtadataResolution {
            active_slot: Slot::from_seq(entry0.ota_seq),
            active_seq: entry0.ota_seq,
            active_sector: OtadataSector::Sector0,
            is_trial: is_trial_state(entry0.ota_state),
        },
        (false, true) => OtadataResolution {
            active_slot: Slot::from_seq(entry1.ota_seq),
            active_seq: entry1.ota_seq,
            active_sector: OtadataSector::Sector1,
            is_trial: is_trial_state(entry1.ota_state),
        },
        (false, false) => OtadataResolution {
            active_slot: Slot::Ota0,
            active_seq: 1,
            active_sector: OtadataSector::Sector0,
            is_trial: false,
        },
    }
}

/// Calculates the next sequence number required to select `target_slot`.
pub const fn next_seq_for_slot(current_seq: u32, target_slot: Slot) -> u32 {
    let target_is_ota0 = matches!(target_slot, Slot::Ota0);
    if current_seq == 0 {
        if target_is_ota0 {
            1
        } else {
            2
        }
    } else {
        let candidate = current_seq.saturating_add(1);
        let candidate_is_ota0 = (candidate - 1).is_multiple_of(2);
        if candidate_is_ota0 == target_is_ota0 {
            candidate
        } else {
            candidate.saturating_add(1)
        }
    }
}

/// Flash access and OTA domain errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashError {
    /// Requested range extends beyond partition boundary.
    OutOfBounds,
    /// Flash address or length does not meet the required alignment.
    AlignmentError,
    /// Physical flash erase failure.
    EraseError,
    /// Physical flash write failure.
    WriteError,
    /// Physical flash read failure.
    ReadError,
    /// otadata entry CRC-32 verification mismatch.
    InvalidCrc,
    /// otadata partition unreadable or corrupted.
    CorruptedOtadata,
}

impl fmt::Display for FlashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds => write!(f, "Flash operation extends out of partition bounds"),
            Self::AlignmentError => {
                write!(
                    f,
                    "Flash address or length does not meet the required alignment"
                )
            }
            Self::EraseError => write!(f, "Hardware flash sector erase failed"),
            Self::WriteError => write!(f, "Hardware flash write failed"),
            Self::ReadError => write!(f, "Hardware flash read failed"),
            Self::InvalidCrc => write!(f, "otadata entry CRC-32 verification failed"),
            Self::CorruptedOtadata => write!(f, "otadata partition unreadable or corrupted"),
        }
    }
}

/// Asynchronous OTA storage interface for partition queries, binary writes, and bootloader rollback control.
#[allow(async_fn_in_trait)]
pub trait OtaStorage {
    /// Returns the currently active boot slot.
    async fn active_slot(&self) -> Slot;

    /// Returns the alternate passive slot ready for receiving an update binary.
    async fn passive_slot(&self) -> Slot {
        self.active_slot().await.other()
    }

    /// Erases a range within the target application slot.
    ///
    /// Both `offset` and `len` must be 4 KiB sector aligned and within `slot.size()`.
    async fn erase_range(&mut self, slot: Slot, offset: u32, len: u32) -> Result<(), FlashError>;

    /// Writes a binary chunk into the target application slot at `offset`.
    ///
    /// Must fit within `slot.size()`.
    async fn write_chunk(&mut self, slot: Slot, offset: u32, data: &[u8])
        -> Result<(), FlashError>;

    /// Marks the target slot for trial boot on the next reset (`ESP_OTA_IMG_PENDING_VERIFY`).
    async fn mark_trial_boot(&mut self, slot: Slot) -> Result<(), FlashError>;

    /// Confirms current slot operation, marking it `ESP_OTA_IMG_VALID` and cancelling rollback.
    async fn mark_valid(&mut self) -> Result<(), FlashError>;
}

/// In-memory mock implementing [`OtaStorage`] for deterministic host-side testing.
#[derive(Debug, Clone)]
pub struct MockOtaStorage {
    otadata: [u8; OTADATA_SIZE as usize],
    erased_sectors_ota0: [bool; (OTA_SLOT_SIZE / FLASH_SECTOR_SIZE) as usize],
    erased_sectors_ota1: [bool; (OTA_SLOT_SIZE / FLASH_SECTOR_SIZE) as usize],
    written_bytes_ota0: usize,
    written_bytes_ota1: usize,
}

impl Default for MockOtaStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl MockOtaStorage {
    /// Creates an empty in-memory mock storage (initialized as erased flash: all 0xFF).
    pub fn new() -> Self {
        Self {
            otadata: [0xFF; OTADATA_SIZE as usize],
            erased_sectors_ota0: [false; (OTA_SLOT_SIZE / FLASH_SECTOR_SIZE) as usize],
            erased_sectors_ota1: [false; (OTA_SLOT_SIZE / FLASH_SECTOR_SIZE) as usize],
            written_bytes_ota0: 0,
            written_bytes_ota1: 0,
        }
    }

    /// Reads the parsed entry for the specified sector.
    pub fn read_sector_entry(&self, sector: OtadataSector) -> EspOtaSelectEntry {
        let offset = sector.offset_in_partition() as usize;
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&self.otadata[offset..offset + 32]);
        EspOtaSelectEntry::from_bytes(&buf)
    }

    /// Overwrites the entry for the specified sector directly.
    pub fn write_sector_entry(&mut self, sector: OtadataSector, entry: &EspOtaSelectEntry) {
        let offset = sector.offset_in_partition() as usize;
        let bytes = entry.to_bytes();
        self.otadata[offset..offset + 32].copy_from_slice(&bytes);
    }

    /// Manually corrupts the CRC of a sector to simulate flash corruption.
    pub fn corrupt_sector_crc(&mut self, sector: OtadataSector) {
        let mut entry = self.read_sector_entry(sector);
        entry.crc ^= 0xDEAD_BEEF;
        self.write_sector_entry(sector, &entry);
    }

    /// Manually updates the boot state of a sector (e.g. to simulate bootloader marking invalid).
    pub fn set_sector_state(&mut self, sector: OtadataSector, state: u32) {
        let mut entry = self.read_sector_entry(sector);
        entry.ota_state = state;
        entry.crc = entry.compute_crc();
        self.write_sector_entry(sector, &entry);
    }

    /// Resolves the current [`OtadataResolution`] from Sector 0 and Sector 1.
    pub fn resolution(&self) -> OtadataResolution {
        let entry0 = self.read_sector_entry(OtadataSector::Sector0);
        let entry1 = self.read_sector_entry(OtadataSector::Sector1);
        resolve_active_slot(&entry0, &entry1)
    }

    /// Returns the total bytes written into the specified slot.
    pub fn written_bytes(&self, slot: Slot) -> usize {
        match slot {
            Slot::Ota0 => self.written_bytes_ota0,
            Slot::Ota1 => self.written_bytes_ota1,
        }
    }
}

impl OtaStorage for MockOtaStorage {
    async fn active_slot(&self) -> Slot {
        self.resolution().active_slot
    }

    async fn erase_range(&mut self, slot: Slot, offset: u32, len: u32) -> Result<(), FlashError> {
        if (offset as u64) + (len as u64) > (slot.size() as u64) {
            return Err(FlashError::OutOfBounds);
        }
        if !offset.is_multiple_of(FLASH_SECTOR_SIZE) || !len.is_multiple_of(FLASH_SECTOR_SIZE) {
            return Err(FlashError::AlignmentError);
        }

        let start_sector = (offset / FLASH_SECTOR_SIZE) as usize;
        let num_sectors = (len / FLASH_SECTOR_SIZE) as usize;
        let sector_flags = match slot {
            Slot::Ota0 => &mut self.erased_sectors_ota0[start_sector..start_sector + num_sectors],
            Slot::Ota1 => &mut self.erased_sectors_ota1[start_sector..start_sector + num_sectors],
        };

        for flag in sector_flags {
            *flag = true;
        }

        Ok(())
    }

    async fn write_chunk(
        &mut self,
        slot: Slot,
        offset: u32,
        data: &[u8],
    ) -> Result<(), FlashError> {
        if (offset as u64) + (data.len() as u64) > (slot.size() as u64) {
            return Err(FlashError::OutOfBounds);
        }
        if !offset.is_multiple_of(4) {
            return Err(FlashError::AlignmentError);
        }

        match slot {
            Slot::Ota0 => self.written_bytes_ota0 += data.len(),
            Slot::Ota1 => self.written_bytes_ota1 += data.len(),
        }

        Ok(())
    }

    async fn mark_trial_boot(&mut self, slot: Slot) -> Result<(), FlashError> {
        let res = self.resolution();
        let next_seq = next_seq_for_slot(res.active_seq, slot);
        let target_sector = res.active_sector.other();

        // Clear target sector (erase to 0xFF)
        let sector_offset = target_sector.offset_in_partition() as usize;
        self.otadata[sector_offset..sector_offset + FLASH_SECTOR_SIZE as usize].fill(0xFF);

        // Write new trial entry
        let entry = EspOtaSelectEntry::new_trial(next_seq);
        self.write_sector_entry(target_sector, &entry);
        Ok(())
    }

    async fn mark_valid(&mut self) -> Result<(), FlashError> {
        let res = self.resolution();
        let target_sector = res.active_sector;

        // Clear target sector (erase to 0xFF)
        let sector_offset = target_sector.offset_in_partition() as usize;
        self.otadata[sector_offset..sector_offset + FLASH_SECTOR_SIZE as usize].fill(0xFF);

        // Write updated valid entry
        let entry = EspOtaSelectEntry::new_valid(res.active_seq);
        self.write_sector_entry(target_sector, &entry);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simple immediate future runner for no_std async unit tests without external runtime dependencies.
    fn block_on<F: core::future::Future>(mut future: F) -> F::Output {
        use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(core::ptr::null(), &VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        let mut pinned = unsafe { core::pin::Pin::new_unchecked(&mut future) };

        match pinned.as_mut().poll(&mut cx) {
            Poll::Ready(res) => res,
            Poll::Pending => panic!("Mock future did not complete synchronously"),
        }
    }

    #[test]
    fn test_ieee_crc32_standard_vectors() {
        // Standard IEEE 802.3 test vector "123456789"
        assert_eq!(crc32_ieee(b"123456789"), 0xCBF4_3926);

        // Empty vector
        assert_eq!(crc32_ieee(b""), 0x0000_0000);

        // Single byte 'a'
        assert_eq!(crc32_ieee(b"a"), 0xE8B7_BE43);
    }

    #[test]
    fn test_slot_geometry_and_bounds() {
        assert_eq!(Slot::Ota0.offset(), 0x020000);
        assert_eq!(Slot::Ota0.size(), 1856 * 1024);
        assert_eq!(Slot::Ota0.size(), 0x1D0000);
        assert_eq!(Slot::Ota0.name(), "ota_0");
        assert_eq!(Slot::Ota0.other(), Slot::Ota1);

        assert_eq!(Slot::Ota1.offset(), 0x1F0000);
        assert_eq!(Slot::Ota1.size(), 1856 * 1024);
        assert_eq!(Slot::Ota1.size(), 0x1D0000);
        assert_eq!(Slot::Ota1.name(), "ota_1");
        assert_eq!(Slot::Ota1.other(), Slot::Ota0);

        // Verify contiguous symmetric alignment without overlap
        assert_eq!(Slot::Ota0.offset() + Slot::Ota0.size(), Slot::Ota1.offset());
        // Verify end of ota_1 fits within 4 MB flash (0x400000)
        assert_eq!(Slot::Ota1.offset() + Slot::Ota1.size(), 0x3C0000);
        assert!(Slot::Ota1.offset() + Slot::Ota1.size() <= 0x400000);

        // Verify 64 KiB MMU flash mapping page alignment
        assert!(Slot::Ota0.offset().is_multiple_of(0x10000));
        assert!(Slot::Ota1.offset().is_multiple_of(0x10000));
    }

    #[test]
    fn test_slot_from_seq_mapping() {
        assert_eq!(Slot::from_seq(0), Slot::Ota0);
        assert_eq!(Slot::from_seq(1), Slot::Ota0);
        assert_eq!(Slot::from_seq(2), Slot::Ota1);
        assert_eq!(Slot::from_seq(3), Slot::Ota0);
        assert_eq!(Slot::from_seq(4), Slot::Ota1);
        assert_eq!(Slot::from_seq(5), Slot::Ota0);
    }

    #[test]
    fn test_next_seq_for_slot() {
        assert_eq!(next_seq_for_slot(0, Slot::Ota0), 1);
        assert_eq!(next_seq_for_slot(0, Slot::Ota1), 2);
        assert_eq!(next_seq_for_slot(1, Slot::Ota1), 2);
        assert_eq!(next_seq_for_slot(1, Slot::Ota0), 3);
        assert_eq!(next_seq_for_slot(2, Slot::Ota0), 3);
        assert_eq!(next_seq_for_slot(2, Slot::Ota1), 4);
        assert_eq!(next_seq_for_slot(u32::MAX, Slot::Ota0), u32::MAX);
    }

    #[test]
    fn test_esp_ota_select_entry_crc_golden_vector() {
        let entry = EspOtaSelectEntry {
            ota_seq: 1,
            seq_label: [0u8; 20],
            ota_state: ESP_OTA_IMG_NEW,
            crc: 0,
        };
        assert_eq!(entry.compute_crc(), 0x4743_989A);

        let new_valid_entry = EspOtaSelectEntry::new_valid(1);
        assert_eq!(new_valid_entry.compute_crc(), 0x4743_989A);
        assert_eq!(new_valid_entry.crc, 0x4743_989A);
    }

    #[test]
    fn test_esp_ota_select_entry_crc_validation() {
        let entry = EspOtaSelectEntry::new_valid(1);
        assert_eq!(entry.crc, 0x4743_989A);
        assert!(entry.is_crc_valid());
        assert!(entry.is_usable());

        let bytes = entry.to_bytes();
        assert_eq!(bytes.len(), 32);
        let parsed = EspOtaSelectEntry::from_bytes(&bytes);
        assert_eq!(parsed, entry);
        assert!(parsed.is_crc_valid());

        // Corrupt sequence -> CRC mismatch
        let mut corrupted = entry;
        corrupted.ota_seq = 2;
        assert!(!corrupted.is_crc_valid());

        // Corrupt CRC explicitly
        let mut corrupted_crc = entry;
        corrupted_crc.crc ^= 0xDEAD_BEEF;
        assert!(!corrupted_crc.is_crc_valid());
        assert!(!corrupted_crc.is_usable());

        // State changes do not affect CRC because CRC only covers ota_seq,
        // but invalid/aborted states make the entry unusable.
        let mut corrupted_state = entry;
        corrupted_state.ota_state = ESP_OTA_IMG_ABORTED;
        assert!(corrupted_state.is_crc_valid());
        assert!(!corrupted_state.is_usable());

        corrupted_state.ota_state = ESP_OTA_IMG_INVALID;
        assert!(corrupted_state.is_crc_valid());
        assert!(!corrupted_state.is_usable());

        // Erased flash (all 0xFF) is invalid
        let erased = EspOtaSelectEntry::from_bytes(&[0xFF; 32]);
        assert!(!erased.is_crc_valid());
        assert!(!erased.is_usable());
    }

    #[test]
    fn test_slot_resolution_scenarios() {
        // Scenario 1: Clean/uninitialized flash -> Slot::Ota0 (seq 1)
        let erased = EspOtaSelectEntry::from_bytes(&[0xFF; 32]);
        let res = resolve_active_slot(&erased, &erased);
        assert_eq!(res.active_slot, Slot::Ota0);
        assert_eq!(res.active_seq, 1);
        assert_eq!(res.active_sector, OtadataSector::Sector0);

        // Scenario 2: Sector 0 valid (seq 1), Sector 1 erased -> Sector 0 wins (Ota0)
        let entry0 = EspOtaSelectEntry::new_valid(1);
        let res = resolve_active_slot(&entry0, &erased);
        assert_eq!(res.active_slot, Slot::Ota0);
        assert_eq!(res.active_seq, 1);
        assert_eq!(res.active_sector, OtadataSector::Sector0);

        // Scenario 3: Sector 0 valid (seq 1), Sector 1 trial (seq 2) -> Sector 1 wins (Ota1)
        let entry1 = EspOtaSelectEntry::new_trial(2);
        let res = resolve_active_slot(&entry0, &entry1);
        assert_eq!(res.active_slot, Slot::Ota1);
        assert_eq!(res.active_seq, 2);
        assert_eq!(res.active_sector, OtadataSector::Sector1);
        assert!(res.is_trial);

        // Also verify ESP_OTA_IMG_PENDING_VERIFY is recognized as trial boot state
        let mut pending_entry = entry1;
        pending_entry.ota_state = ESP_OTA_IMG_PENDING_VERIFY;
        let res_pending = resolve_active_slot(&entry0, &pending_entry);
        assert_eq!(res_pending.active_slot, Slot::Ota1);
        assert!(res_pending.is_trial);

        // Scenario 4: Rollback - Sector 1 has higher sequence (seq 2) but is marked INVALID
        let mut invalid_entry1 = entry1;
        invalid_entry1.ota_state = ESP_OTA_IMG_INVALID;
        invalid_entry1.crc = invalid_entry1.compute_crc();
        let res = resolve_active_slot(&entry0, &invalid_entry1);
        assert_eq!(res.active_slot, Slot::Ota0);
        assert_eq!(res.active_seq, 1);
        assert_eq!(res.active_sector, OtadataSector::Sector0);

        // Scenario 5: Corrupt Sector 1 CRC -> Sector 0 wins
        let mut corrupt_crc_entry1 = entry1;
        corrupt_crc_entry1.crc ^= 0x1234;
        let res = resolve_active_slot(&entry0, &corrupt_crc_entry1);
        assert_eq!(res.active_slot, Slot::Ota0);
        assert_eq!(res.active_sector, OtadataSector::Sector0);
    }

    #[test]
    fn test_mock_ota_storage_full_lifecycle_and_rollback() {
        let mut storage = MockOtaStorage::new();

        // 1. Initial boot: Clean flash resolves to Ota0
        assert_eq!(block_on(storage.active_slot()), Slot::Ota0);
        assert_eq!(block_on(storage.passive_slot()), Slot::Ota1);

        // 2. Erase passive slot (ota_1) and write firmware chunks
        let erase_res = block_on(storage.erase_range(Slot::Ota1, 0, 8192));
        assert!(erase_res.is_ok());

        let write_res = block_on(storage.write_chunk(Slot::Ota1, 0, &[0xAA; 1024]));
        assert!(write_res.is_ok());
        assert_eq!(storage.written_bytes(Slot::Ota1), 1024);

        // 3. Mark trial boot for ota_1
        let trial_res = block_on(storage.mark_trial_boot(Slot::Ota1));
        assert!(trial_res.is_ok());

        // Now active slot is Ota1 in trial mode (NEW / PENDING_VERIFY)
        assert_eq!(block_on(storage.active_slot()), Slot::Ota1);
        assert_eq!(block_on(storage.passive_slot()), Slot::Ota0);
        assert!(storage.resolution().is_trial);

        // 4. Mark valid (health check passed)
        let valid_res = block_on(storage.mark_valid());
        assert!(valid_res.is_ok());
        assert_eq!(block_on(storage.active_slot()), Slot::Ota1);
        assert!(!storage.resolution().is_trial);

        // 5. Next update: Flash into ota_0 and mark trial boot
        let trial2_res = block_on(storage.mark_trial_boot(Slot::Ota0));
        assert!(trial2_res.is_ok());
        assert_eq!(block_on(storage.active_slot()), Slot::Ota0);
        assert!(storage.resolution().is_trial);

        // 6. Simulate rollback: Image in ota_0 fails boot and bootloader marks it INVALID
        let active_sector = storage.resolution().active_sector;
        storage.set_sector_state(active_sector, ESP_OTA_IMG_INVALID);

        // Active slot must roll back to Ota1!
        assert_eq!(block_on(storage.active_slot()), Slot::Ota1);
        assert_eq!(block_on(storage.passive_slot()), Slot::Ota0);
    }

    #[test]
    fn test_storage_boundary_and_alignment_validation() {
        let mut storage = MockOtaStorage::new();

        // Out of bounds erase
        let err = block_on(storage.erase_range(Slot::Ota0, OTA_SLOT_SIZE, 4096)).unwrap_err();
        assert_eq!(err, FlashError::OutOfBounds);

        let err =
            block_on(storage.erase_range(Slot::Ota0, OTA_SLOT_SIZE - 2048, 4096)).unwrap_err();
        assert_eq!(err, FlashError::OutOfBounds);

        // Unaligned erase (not 4 KiB aligned)
        let err = block_on(storage.erase_range(Slot::Ota0, 100, 4096)).unwrap_err();
        assert_eq!(err, FlashError::AlignmentError);

        let err = block_on(storage.erase_range(Slot::Ota0, 0, 1000)).unwrap_err();
        assert_eq!(err, FlashError::AlignmentError);

        // Out of bounds write
        let err = block_on(storage.write_chunk(Slot::Ota0, OTA_SLOT_SIZE, &[1, 2, 3])).unwrap_err();
        assert_eq!(err, FlashError::OutOfBounds);

        let err = block_on(storage.write_chunk(Slot::Ota0, OTA_SLOT_SIZE - 2, &[1, 2, 3, 4]))
            .unwrap_err();
        assert_eq!(err, FlashError::OutOfBounds);

        // Unaligned write offset (not 4-byte aligned)
        let err = block_on(storage.write_chunk(Slot::Ota0, 1, &[1, 2, 3, 4])).unwrap_err();
        assert_eq!(err, FlashError::AlignmentError);
        let err = block_on(storage.write_chunk(Slot::Ota0, 3, &[1, 2, 3, 4])).unwrap_err();
        assert_eq!(err, FlashError::AlignmentError);

        // Valid write at boundary edge
        let ok = block_on(storage.write_chunk(Slot::Ota0, OTA_SLOT_SIZE - 4, &[1, 2, 3, 4]));
        assert!(ok.is_ok());
    }
}
