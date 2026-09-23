//! Streaming verified update session.
//!
//! Pure and allocation-free. [`apply_update`] streams a bounded body from a
//! [`BodyReader`] into a [`Flasher`] while hashing it with streaming SHA-256,
//! and only commits once every byte has been written and the digest matches.

#![allow(async_fn_in_trait)]

use sha2::{Digest, Sha256};

use crate::ota::OTA_SLOT_SIZE;

use super::CHUNK_BYTES;

/// Sink for a streamed image: positional writes plus a final commit.
pub trait Flasher {
    type Error;

    async fn write(&mut self, offset: usize, data: &[u8]) -> Result<(), Self::Error>;

    async fn mark_updated(&mut self) -> Result<(), Self::Error>;
}

/// Source of an image body; `Ok(0)` signals EOF.
pub trait BodyReader {
    type Error;

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error>;
}

/// Reasons [`apply_update`] can reject or abort an image.
#[derive(Debug, PartialEq, Eq)]
pub enum UpdateError<FE, RE> {
    /// The declared body length exceeds the slot capacity.
    Oversize { length: u64, capacity: u32 },
    /// The body ended before `content_length` bytes were read.
    TooShort,
    /// The reader claimed more bytes than the requested buffer length.
    TooLong,
    /// The streamed SHA-256 digest did not match `expected`.
    HashMismatch,
    /// A [`Flasher`] write or commit failed.
    Flash(FE),
    /// A [`BodyReader`] read failed.
    Read(RE),
}

/// Streams `content_length` bytes from `body` to `flasher`, verifying SHA-256.
///
/// Ordering invariant: an image larger than [`OTA_SLOT_SIZE`] is rejected before
/// any `flasher` call, and [`Flasher::mark_updated`] is only reached after every
/// chunk has been written and the digest matches `expected`. No error path marks
/// the DFU.
pub async fn apply_update<F: Flasher, R: BodyReader>(
    flasher: &mut F,
    body: &mut R,
    content_length: u64,
    expected: &[u8; 32],
) -> Result<(), UpdateError<F::Error, R::Error>> {
    if content_length > OTA_SLOT_SIZE as u64 {
        return Err(UpdateError::Oversize {
            length: content_length,
            capacity: OTA_SLOT_SIZE,
        });
    }

    let mut buf = [0u8; CHUNK_BYTES];
    let mut hasher = Sha256::new();
    let mut offset = 0usize;
    let mut total: u64 = 0;
    // Read exactly `content_length` bytes: never issue a read past the body.
    // A read after the last byte can error on connection close (the server may
    // drop the socket without a TLS close_notify), which must not fail an
    // otherwise complete, verified image.
    while total < content_length {
        let remaining = (content_length - total).min(buf.len() as u64) as usize;
        let n = body
            .read(&mut buf[..remaining])
            .await
            .map_err(UpdateError::Read)?;
        if n == 0 {
            break;
        }
        if n > remaining {
            return Err(UpdateError::TooLong);
        }
        total += n as u64;
        hasher.update(&buf[..n]);
        flasher
            .write(offset, &buf[..n])
            .await
            .map_err(UpdateError::Flash)?;
        offset += n;
    }

    if total != content_length {
        return Err(UpdateError::TooShort);
    }
    let digest = hasher.finalize();
    if digest[..] != expected[..] {
        return Err(UpdateError::HashMismatch);
    }

    flasher.mark_updated().await.map_err(UpdateError::Flash)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Records every write and counts commits.
    #[derive(Default)]
    struct MockFlasher {
        writes: Vec<(usize, Vec<u8>)>,
        mark_updated_calls: usize,
    }

    impl Flasher for MockFlasher {
        type Error = ();

        async fn write(&mut self, offset: usize, data: &[u8]) -> Result<(), ()> {
            self.writes.push((offset, data.to_vec()));
            Ok(())
        }

        async fn mark_updated(&mut self) -> Result<(), ()> {
            self.mark_updated_calls += 1;
            Ok(())
        }
    }

    /// Yields `data`; tracks how many reads were issued and bytes requested.
    struct MockReader {
        data: Vec<u8>,
        pos: usize,
        forced_len: Option<usize>,
        read_calls: usize,
        total_requested: usize,
    }

    impl MockReader {
        fn new(data: Vec<u8>) -> Self {
            Self {
                data,
                pos: 0,
                forced_len: None,
                read_calls: 0,
                total_requested: 0,
            }
        }
    }

    impl BodyReader for MockReader {
        type Error = ();

        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
            self.read_calls += 1;
            self.total_requested += buf.len();
            if let Some(forced) = self.forced_len {
                return Ok(forced);
            }
            let remaining = self.data.len() - self.pos;
            let n = remaining.min(buf.len());
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn oversize_is_rejected_before_any_write() {
        let mut flasher = MockFlasher::default();
        let mut body = MockReader::new(vec![0u8; 16]);
        let length = OTA_SLOT_SIZE as u64 + 1;
        let err = block_on(apply_update(&mut flasher, &mut body, length, &[0u8; 32])).unwrap_err();
        assert_eq!(
            err,
            UpdateError::Oversize {
                length,
                capacity: OTA_SLOT_SIZE,
            }
        );
        assert!(flasher.writes.is_empty());
        assert_eq!(flasher.mark_updated_calls, 0);
        assert_eq!(body.read_calls, 0);
    }

    #[test]
    fn matching_body_writes_contiguously_and_commits_once() {
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let expected: [u8; 32] = Sha256::digest(&data).into();
        let mut flasher = MockFlasher::default();
        let mut body = MockReader::new(data.clone());

        block_on(apply_update(
            &mut flasher,
            &mut body,
            data.len() as u64,
            &expected,
        ))
        .unwrap();

        assert_eq!(flasher.mark_updated_calls, 1);
        assert_eq!(body.total_requested, data.len());
        assert_eq!(body.read_calls, 3);
        assert_eq!(flasher.writes.len(), 3);

        let mut offset = 0usize;
        let mut streamed = Vec::new();
        for (chunk_offset, chunk) in &flasher.writes {
            assert_eq!(*chunk_offset, offset);
            offset += chunk.len();
            streamed.extend_from_slice(chunk);
        }
        assert_eq!(offset, data.len());
        assert_eq!(streamed, data);
    }

    #[test]
    fn hash_mismatch_never_commits() {
        let data = vec![0xABu8; 100];
        let expected = [0u8; 32];
        let mut flasher = MockFlasher::default();
        let mut body = MockReader::new(data.clone());

        let err = block_on(apply_update(
            &mut flasher,
            &mut body,
            data.len() as u64,
            &expected,
        ))
        .unwrap_err();

        assert_eq!(err, UpdateError::HashMismatch);
        assert_eq!(flasher.mark_updated_calls, 0);
    }

    #[test]
    fn short_body_is_too_short() {
        let data = vec![0x5Au8; 50];
        let expected: [u8; 32] = Sha256::digest(&data).into();
        let mut flasher = MockFlasher::default();
        let mut body = MockReader::new(data);

        let err = block_on(apply_update(&mut flasher, &mut body, 100, &expected)).unwrap_err();

        assert_eq!(err, UpdateError::TooShort);
        assert_eq!(flasher.mark_updated_calls, 0);
    }

    #[test]
    fn reader_overrun_is_too_long() {
        let data = vec![0x11u8; 64];
        let expected: [u8; 32] = Sha256::digest(&data).into();
        let mut flasher = MockFlasher::default();
        let mut body = MockReader::new(data);
        body.forced_len = Some(11);

        let err = block_on(apply_update(&mut flasher, &mut body, 10, &expected)).unwrap_err();

        assert_eq!(err, UpdateError::TooLong);
        assert_eq!(flasher.mark_updated_calls, 0);
    }

    #[test]
    fn no_read_is_issued_past_content_length() {
        let data = vec![0x42u8; 10];
        let expected: [u8; 32] = Sha256::digest(&data).into();
        let mut flasher = MockFlasher::default();
        let mut body = MockReader::new(data.clone());

        block_on(apply_update(
            &mut flasher,
            &mut body,
            data.len() as u64,
            &expected,
        ))
        .unwrap();

        assert_eq!(body.read_calls, 1);
        assert_eq!(body.total_requested, data.len());
        assert_eq!(flasher.mark_updated_calls, 1);
    }
}
