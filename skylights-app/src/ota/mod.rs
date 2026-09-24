//! OTA trigger seam and streaming update task for the Skylights controller.
//!
//! [`OTA_TRIGGER`] is raised by `skylight/reset` (via `mqtt.rs`) and once at
//! boot by `main.rs`. [`ota_task`] waits on it and runs one update check. The
//! check fetches `version`, `{v}.bin.sha256` and `{v}.bin` over TLS, streams the
//! image into the passive slot while hashing it, and only marks a trial boot
//! once the full SHA-256 matches. A transient failure is logged as a stable
//! `code=` and the task waits again; the chip is reset only after a verified
//! image has been committed to `otadata`.

pub mod selftest;
mod transport;

use core::fmt::Write as _;

use embassy_executor::task;
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use esp_println::println;
use skylights_core::ota::session::{apply_update, Flasher, UpdateError};
use skylights_core::ota::{
    decide, parse_sha256_hex, parse_url, parse_version, Decision, Slot, FLASH_SECTOR_SIZE,
};
use skylights_core::{FlashError, OtaStorage};
use static_cell::StaticCell;

use crate::flash::EspFlashStorage;
use crate::secrets;
use transport::{
    build_request, open, resolve, FetchError, HttpBody, OtaError, TextBuf, READ_BUF, READ_CHUNK,
    RECORD_BYTES, REC_READ, REC_WRITE, TCP_BYTES, TCP_RX, TCP_TX, WRITE_RECORD_BYTES,
};

/// Raised when `skylight/reset` requests an OTA check (and once at boot).
pub static OTA_TRIGGER: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Upper bound on the `version` text body.
const VERSION_MAX_BYTES: usize = 64;
/// Upper bound on the `{v}.bin.sha256` text body.
const SHA_MAX_BYTES: usize = 128;
/// Upper bound on a `{v}.bin` / `{v}.bin.sha256` request name.
const NAME_MAX_BYTES: usize = 48;

/// Writes a streamed image into one passive slot through [`OtaStorage`].
///
/// Flash is programmed in **whole 4 KiB sectors at 4 KiB-aligned offsets**. That
/// is required, not cosmetic: esp-storage's ESP32 write programs 32-byte blocks
/// at `addr + 32k` and never splits at the 256-byte flash page boundary, so a
/// write starting at a non-page-aligned offset shifts data. The HTTP stream
/// arrives in arbitrary chunk sizes, so we buffer it into full sectors; every
/// flash write is then page-safe.
struct SlotFlasher<'a> {
    storage: &'a mut EspFlashStorage,
    slot: Slot,
    buf: &'a mut [u8],
    len: usize,
    sector: u32,
    next_log: u32,
}

impl<'a> SlotFlasher<'a> {
    fn new(storage: &'a mut EspFlashStorage, slot: Slot, buf: &'a mut [u8]) -> Self {
        debug_assert_eq!(buf.len(), FLASH_SECTOR_SIZE as usize);
        Self {
            storage,
            slot,
            buf,
            len: 0,
            sector: 0,
            next_log: 128 * 1024,
        }
    }

    /// Erases and writes the buffered sector (padded to a 4-byte word with
    /// 0xFF), then advances to the next sector.
    async fn flush_sector(&mut self) -> Result<(), FlashError> {
        let mut end = self.len;
        while !end.is_multiple_of(4) {
            self.buf[end] = 0xFF;
            end += 1;
        }
        let offset = self.sector * FLASH_SECTOR_SIZE;
        self.storage
            .erase_range(self.slot, offset, FLASH_SECTOR_SIZE)
            .await?;
        self.storage
            .write_chunk(self.slot, offset, &self.buf[..end])
            .await?;

        self.sector += 1;
        self.len = 0;
        let written = self.sector * FLASH_SECTOR_SIZE;
        if written >= self.next_log {
            println!("OTA: wrote {} KiB", written / 1024);
            self.next_log = written + 128 * 1024;
        }
        Ok(())
    }

    /// Erases the passive slot's first sector so a rejected image is never
    /// selectable. `otadata` is left untouched.
    async fn abort(&mut self) -> Result<(), FlashError> {
        self.storage
            .erase_range(self.slot, 0, FLASH_SECTOR_SIZE)
            .await
    }
}

impl Flasher for SlotFlasher<'_> {
    type Error = FlashError;

    async fn write(&mut self, _offset: usize, data: &[u8]) -> Result<(), Self::Error> {
        let mut data = data;
        while !data.is_empty() {
            let space = self.buf.len() - self.len;
            let take = space.min(data.len());
            self.buf[self.len..self.len + take].copy_from_slice(&data[..take]);
            self.len += take;
            data = &data[take..];
            if self.len == self.buf.len() {
                self.flush_sector().await?;
            }
        }
        Ok(())
    }

    async fn mark_updated(&mut self) -> Result<(), Self::Error> {
        if self.len > 0 {
            self.flush_sector().await?;
        }
        self.storage.mark_trial_boot(self.slot).await
    }
}

/// 4 KiB staging buffer for one whole flash sector (see [`SlotFlasher`]).
static SECTOR_BUF: StaticCell<[u8; FLASH_SECTOR_SIZE as usize]> = StaticCell::new();

/// Waits for [`OTA_TRIGGER`] and runs one update check per signal.
///
/// The large transport buffers are `StaticCell`-backed and initialised once
/// before the loop so the task future stays small enough for the shared task
/// arena. A failed check is logged and the task waits again; it never resets
/// except after [`check_and_update`] has committed a verified image.
#[task]
pub async fn ota_task(stack: Stack<'static>, rng: esp_hal::rng::Rng) -> ! {
    let tcp_rx = TCP_RX.init([0; TCP_BYTES]);
    let tcp_tx = TCP_TX.init([0; TCP_BYTES]);
    let rec_read = REC_READ.init([0; RECORD_BYTES]);
    let rec_write = REC_WRITE.init([0; WRITE_RECORD_BYTES]);
    let leftover = READ_BUF.init([0; READ_CHUNK]);
    let sector_buf = SECTOR_BUF.init([0; FLASH_SECTOR_SIZE as usize]);

    let mut storage = EspFlashStorage::new();

    loop {
        OTA_TRIGGER.wait().await;
        match check_and_update(
            stack,
            rng,
            &mut storage,
            &mut *tcp_rx,
            &mut *tcp_tx,
            &mut *rec_read,
            &mut *rec_write,
            &mut *leftover,
            &mut *sector_buf,
        )
        .await
        {
            Ok(()) => {}
            Err(error) => println!("OTA: failed code={}", error.code()),
        }
    }
}

/// Resolves, fetches, verifies and (only then) marks one image.
///
/// Returns `Ok(())` for every transient condition (404, no update, missing
/// body) as well as for the errors the caller logs. Never loops, never resets.
#[allow(clippy::too_many_arguments)]
async fn check_and_update(
    stack: Stack<'static>,
    rng: esp_hal::rng::Rng,
    storage: &mut EspFlashStorage,
    tcp_rx: &mut [u8],
    tcp_tx: &mut [u8],
    rec_read: &mut [u8],
    rec_write: &mut [u8],
    leftover: &mut [u8],
    sector_buf: &mut [u8],
) -> Result<(), OtaError> {
    stack.wait_config_up().await;

    let uri = parse_url(secrets::OTA_URL, secrets::OTA_PROJECT).map_err(|_| {
        println!("OTA: invalid base url");
        OtaError::Url
    })?;
    let endpoint = resolve(stack, &uri).await?;

    let remote = {
        let request = build_request(&uri, "version")?;
        let mut link = open(
            stack, endpoint, &uri, rng, tcp_rx, tcp_tx, rec_read, rec_write,
        )
        .await?;
        let (head, extra) = link
            .send(request.as_bytes(), leftover)
            .await
            .map_err(fetch_failed)?;
        if head.status != 200 {
            println!("OTA: version missing (status {})", head.status);
            return Ok(());
        }
        let len = head.content_length.ok_or(OtaError::Head)?;
        if len as usize > VERSION_MAX_BYTES {
            println!("OTA: version too long ({})", len);
            return Err(OtaError::Head);
        }
        let mut body = [0u8; VERSION_MAX_BYTES];
        let n = link
            .read_small(&leftover[..extra], len, &mut body)
            .await
            .map_err(fetch_failed)?;
        parse_version(&body[..n]).ok_or_else(|| {
            println!("OTA: invalid version body");
            OtaError::Version
        })?
    };

    let local = parse_version(env!("SKYLIGHTS_BUILD_VERSION").as_bytes()).unwrap_or(0);
    match decide(local, remote) {
        Decision::Skip => {
            println!(
                "OTA: version up to date (local={} remote={})",
                local, remote
            );
            return Ok(());
        }
        Decision::Update => println!("OTA: update available local={} remote={}", local, remote),
    }

    let expected = {
        let mut name = TextBuf::<NAME_MAX_BYTES>::new();
        write!(name, "{}.bin.sha256", remote).map_err(|_| OtaError::Request)?;
        let request = build_request(&uri, name.as_str())?;
        let mut link = open(
            stack, endpoint, &uri, rng, tcp_rx, tcp_tx, rec_read, rec_write,
        )
        .await?;
        let (head, extra) = link
            .send(request.as_bytes(), leftover)
            .await
            .map_err(fetch_failed)?;
        if head.status != 200 {
            println!("OTA: sha missing (status {})", head.status);
            return Ok(());
        }
        let len = head.content_length.ok_or(OtaError::Head)?;
        if len as usize > SHA_MAX_BYTES {
            println!("OTA: sha too long ({})", len);
            return Err(OtaError::Head);
        }
        let mut body = [0u8; SHA_MAX_BYTES];
        let n = link
            .read_small(&leftover[..extra], len, &mut body)
            .await
            .map_err(fetch_failed)?;
        parse_sha256_hex(&body[..n]).ok_or_else(|| {
            println!("OTA: invalid sha body");
            OtaError::Sha
        })?
    };

    let mut name = TextBuf::<NAME_MAX_BYTES>::new();
    write!(name, "{}.bin", remote).map_err(|_| OtaError::Request)?;
    let request = build_request(&uri, name.as_str())?;
    let mut link = open(
        stack, endpoint, &uri, rng, tcp_rx, tcp_tx, rec_read, rec_write,
    )
    .await?;
    let (head, extra) = link
        .send(request.as_bytes(), leftover)
        .await
        .map_err(fetch_failed)?;
    if head.status != 200 {
        println!("OTA: bin missing (status {})", head.status);
        return Ok(());
    }
    let len = head.content_length.ok_or(OtaError::Head)?;
    println!("OTA: bin status={} len={}", head.status, len);

    let passive = storage.passive_slot().await;
    let mut flasher = SlotFlasher::new(storage, passive, sector_buf);

    let result = {
        let mut body = HttpBody {
            link: &mut link,
            leftover: &leftover[..extra],
        };
        apply_update(&mut flasher, &mut body, len, &expected).await
    };

    if let Err(error) = result {
        if matches!(error, UpdateError::HashMismatch) {
            println!("OTA: hash mismatch, erasing passive header");
            let _ = flasher.abort().await;
        }
        return Err(OtaError::Update(update_code(&error)));
    }

    println!("OTA: ota_hash_ok");
    println!("OTA: ota_marked resetting");
    esp_hal::reset::software_reset();
    Ok(())
}

/// Logs a transport failure and wraps it for the task's stable error code.
fn fetch_failed(error: FetchError) -> OtaError {
    println!("OTA: fetch failed code={}", error.code());
    OtaError::Transport(error)
}

/// Maps an [`UpdateError`] to a stable, secret-free code.
fn update_code<FE, RE>(error: &UpdateError<FE, RE>) -> &'static str {
    match error {
        UpdateError::Oversize { .. } => "oversize",
        UpdateError::TooShort => "too_short",
        UpdateError::TooLong => "too_long",
        UpdateError::HashMismatch => "hash_mismatch",
        UpdateError::Flash(_) => "flash",
        UpdateError::Read(_) => "read",
    }
}
