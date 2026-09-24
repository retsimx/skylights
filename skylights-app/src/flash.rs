//! Physical flash driver implementing [`OtaStorage`] for ESP32 hardware via [`esp_storage::FlashStorage`].

use core::cell::RefCell;
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_storage::FlashStorage;
use skylights_core::ota::{
    next_seq_for_slot, resolve_active_slot, EspOtaSelectEntry, FlashError, OtaStorage,
    OtadataResolution, OtadataSector, Slot, ESP_OTA_IMG_INVALID, FLASH_SECTOR_SIZE,
};

/// Hardware SPI flash storage driver for ESP32 OTA updates.
pub struct EspFlashStorage {
    flash: RefCell<FlashStorage>,
}

impl Default for EspFlashStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl EspFlashStorage {
    /// Creates a new physical flash storage driver instance.
    pub fn new() -> Self {
        Self {
            flash: RefCell::new(FlashStorage::new()),
        }
    }

    /// Reads and parses the 32-byte descriptor from the specified otadata sector.
    pub fn read_sector_entry(
        &self,
        sector: OtadataSector,
    ) -> Result<EspOtaSelectEntry, FlashError> {
        let mut buf = [0u8; 32];
        let offset = sector.flash_offset();

        self.flash
            .borrow_mut()
            .read(offset, &mut buf)
            .map_err(|_| FlashError::ReadError)?;

        Ok(EspOtaSelectEntry::from_bytes(&buf))
    }

    /// Resolves the current boot slot state by reading and arbitrating both otadata sectors.
    pub fn resolve_otadata(&self) -> Result<OtadataResolution, FlashError> {
        let entry0 = self.read_sector_entry(OtadataSector::Sector0)?;
        let entry1 = self.read_sector_entry(OtadataSector::Sector1)?;
        Ok(resolve_active_slot(&entry0, &entry1))
    }
}

impl OtaStorage for EspFlashStorage {
    async fn active_slot(&self) -> Slot {
        match self.resolve_otadata() {
            Ok(res) => res.active_slot,
            Err(_) => Slot::Ota0,
        }
    }

    async fn erase_range(&mut self, slot: Slot, offset: u32, len: u32) -> Result<(), FlashError> {
        if (offset as u64) + (len as u64) > (slot.size() as u64) {
            return Err(FlashError::OutOfBounds);
        }
        if !offset.is_multiple_of(FLASH_SECTOR_SIZE) || !len.is_multiple_of(FLASH_SECTOR_SIZE) {
            return Err(FlashError::AlignmentError);
        }

        let flash_start = slot.offset() + offset;
        let flash_end = flash_start + len;

        self.flash
            .borrow_mut()
            .erase(flash_start, flash_end)
            .map_err(|_| FlashError::EraseError)
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
        if data.is_empty() {
            return Ok(());
        }
        if !offset.is_multiple_of(4) {
            return Err(FlashError::AlignmentError);
        }

        let flash_addr = slot.offset() + offset;
        let aligned_len = data.len() - (data.len() % 4);
        let mut flash = self.flash.borrow_mut();

        if aligned_len > 0 {
            flash
                .write(flash_addr, &data[..aligned_len])
                .map_err(|_| FlashError::WriteError)?;
        }

        let remainder = &data[aligned_len..];
        if !remainder.is_empty() {
            let mut word = [0xFFu8; 4];
            word[..remainder.len()].copy_from_slice(remainder);
            flash
                .write(flash_addr + aligned_len as u32, &word)
                .map_err(|_| FlashError::WriteError)?;
        }

        Ok(())
    }

    async fn mark_trial_boot(&mut self, slot: Slot) -> Result<(), FlashError> {
        let res = self.resolve_otadata()?;
        let next_seq = next_seq_for_slot(res.active_seq, slot);
        let target_sector = res.active_sector.other();
        let sector_addr = target_sector.flash_offset();
        let mut flash = self.flash.borrow_mut();

        // 1. Erase target sector (4096 bytes)
        flash
            .erase(sector_addr, sector_addr + FLASH_SECTOR_SIZE)
            .map_err(|_| FlashError::EraseError)?;

        // 2. Prepare new trial entry
        let entry = EspOtaSelectEntry::new_trial(next_seq);
        let bytes = entry.to_bytes();

        // 3. Write entry to beginning of sector
        flash
            .write(sector_addr, &bytes)
            .map_err(|_| FlashError::WriteError)?;

        Ok(())
    }

    async fn mark_valid(&mut self) -> Result<(), FlashError> {
        let res = self.resolve_otadata()?;
        let target_sector = res.active_sector;
        let sector_addr = target_sector.flash_offset();
        let mut flash = self.flash.borrow_mut();

        // 1. Erase target sector (4096 bytes)
        flash
            .erase(sector_addr, sector_addr + FLASH_SECTOR_SIZE)
            .map_err(|_| FlashError::EraseError)?;

        // 2. Prepare confirmed valid entry
        let entry = EspOtaSelectEntry::new_valid(res.active_seq);
        let bytes = entry.to_bytes();

        // 3. Write entry to beginning of sector
        flash
            .write(sector_addr, &bytes)
            .map_err(|_| FlashError::WriteError)?;

        Ok(())
    }

    async fn mark_invalid(&mut self) -> Result<(), FlashError> {
        let res = self.resolve_otadata()?;
        let target_sector = res.active_sector;
        let sector_addr = target_sector.flash_offset();
        let mut flash = self.flash.borrow_mut();

        // 1. Erase target sector (4096 bytes)
        flash
            .erase(sector_addr, sector_addr + FLASH_SECTOR_SIZE)
            .map_err(|_| FlashError::EraseError)?;

        // 2. Prepare an INVALID entry for the same sequence so arbitration falls
        //    back to the other slot.
        let mut entry = EspOtaSelectEntry::new_valid(res.active_seq);
        entry.ota_state = ESP_OTA_IMG_INVALID;
        entry.crc = entry.compute_crc();
        let bytes = entry.to_bytes();

        // 3. Write entry to beginning of sector
        flash
            .write(sector_addr, &bytes)
            .map_err(|_| FlashError::WriteError)?;

        Ok(())
    }
}
