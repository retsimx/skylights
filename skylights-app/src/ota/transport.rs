//! OTA transport: DNS, TCP, TLS and the HTTP/1.0 request/response plumbing.
//!
//! Every request opens a fresh connection and the server is told
//! `Connection: close`; there is no connection reuse. [`Opened`] is either a
//! plain [`TcpSocket`] or a [`TlsConnection`] over one, so the request sequence
//! is written once.
//!
//! The transport uses no-verify TLS 1.3 encryption and relies on the
//! end-to-end SHA-256 image hash as the integrity gate (see the SL-7 design
//! note). RSA signature schemes are still advertised so RSA-certificate
//! endpoints negotiate.

use core::fmt::Write as _;
use core::net::Ipv4Addr;

use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, IpEndpoint, Stack};
use embassy_time::{with_timeout, Duration};
use embedded_io_async::{Read, Write};
use embedded_tls::{
    Aes128GcmSha256, CryptoProvider, CryptoRngCore, TlsConfig, TlsConnection, TlsContext,
};
use skylights_core::ota::http::{HeadError, HeadEvent, HeadParser, ResponseHead};
use skylights_core::ota::session::BodyReader;
use skylights_core::ota::{basic_authorization, Uri};
use static_cell::StaticCell;

use crate::secrets;

/// DNS lookup deadline.
const DNS_TIMEOUT: Duration = Duration::from_secs(10);
/// TCP connect deadline.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// TLS 1.3 handshake deadline.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
/// One read/write deadline; a healthy peer answers well inside this.
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Bytes read from the transport while feeding the head parser. Bounds the
/// body that can be inadvertently over-read into the leftover buffer.
pub(super) const READ_CHUNK: usize = 256;
/// TLS read record buffer size. A TLS 1.3 record can be up to 2^14 (16384)
/// plaintext plus up to 256 bytes of AEAD/content-type overhead, so the buffer
/// must be at least 16384 + 256 = 16640; a 16384-byte buffer fails on a
/// full-size record. 17 KiB leaves margin.
pub(super) const RECORD_BYTES: usize = 17 * 1024;
/// TLS write record buffer size.
///
/// The client only ever sends small records: the ClientHello (fixed extensions
/// plus one P-256 key share, well under 512 bytes), the Finished, and a short
/// GET request (`REQUEST_MAX_BYTES`). `embedded-tls` encodes the ClientHello
/// into the write buffer, so 4 KiB is ample. Only the read path must hold a
/// full-size incoming record, which is why `RECORD_BYTES` stays 17 KiB. The
/// symmetric 17 KiB buffers plus the 96 KiB app heap overflow the 176 KiB
/// ESP32 `dram_seg` by ~7 KiB; shrinking the write half is the least-invasive
/// way to fit.
pub(super) const WRITE_RECORD_BYTES: usize = 4 * 1024;
/// TCP receive/transmit window. The window bounds the number of round trips and
/// therefore the sustained TLS read throughput; 2 KiB stalls a large download.
pub(super) const TCP_BYTES: usize = 8192;

/// Bound on the base64 `user:pass` credential buffer.
const AUTH_MAX_BYTES: usize = 192;
/// Bound on one serialized HTTP/1.0 request.
const REQUEST_MAX_BYTES: usize = 512;

/// Network buffers. They live in `.bss` rather than in the task future: the
/// executor's shared task arena is only 8 KiB, so ~50 KiB of stack-local
/// buffers would overflow it. The OTA task initialises each cell exactly once
/// and reborrows the slices per check.
pub(super) static TCP_RX: StaticCell<[u8; TCP_BYTES]> = StaticCell::new();
pub(super) static TCP_TX: StaticCell<[u8; TCP_BYTES]> = StaticCell::new();
pub(super) static REC_READ: StaticCell<[u8; RECORD_BYTES]> = StaticCell::new();
pub(super) static REC_WRITE: StaticCell<[u8; WRITE_RECORD_BYTES]> = StaticCell::new();
pub(super) static READ_BUF: StaticCell<[u8; READ_CHUNK]> = StaticCell::new();

/// `esp_hal::rng::Rng` over the ESP32 hardware RNG, promoted to a
/// `rand_core::CryptoRng`.
///
/// `esp-hal` 0.23 implements `RngCore` for `Rng` but marks only `Trng` (which
/// additionally occupies the ADC) as `CryptoRng`, so `embedded-tls` rejects a
/// bare `Rng`. The ESP32 hardware RNG is the entropy source the IDF uses once
/// Wi-Fi is enabled, so the marker is sound here.
pub(super) struct EspRng(esp_hal::rng::Rng);

impl EspRng {
    /// Wraps the peripheral driver.
    pub(super) fn new(rng: esp_hal::rng::Rng) -> Self {
        Self(rng)
    }
}

impl rand_core::RngCore for EspRng {
    fn next_u32(&mut self) -> u32 {
        self.0.next_u32()
    }

    fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.fill_bytes(dest)
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.0.try_fill_bytes(dest)
    }
}

impl rand_core::CryptoRng for EspRng {}

/// Crypto provider that skips certificate verification and client-cert signing.
///
/// `embedded_tls::UnsecureProvider` behaves identically (it never overrides
/// `verifier`/`signer`, and the handshake treats their `Err` as "skip"), but it
/// hard-wires `Signature = p256::ecdsa::DerSignature`, which links the whole
/// ECDSA/P-256 stack even though it is never executed. Replacing the signature
/// type with a trivial one keeps the same no-verify behaviour without the dead
/// crypto, which matters for the debug-profile FLASH budget.
pub(super) struct NoVerifyProvider<RNG> {
    rng: RNG,
}

impl<RNG: CryptoRngCore> CryptoProvider for NoVerifyProvider<RNG> {
    type CipherSuite = Aes128GcmSha256;
    type Signature = [u8; 64];

    fn rng(&mut self) -> impl CryptoRngCore {
        &mut self.rng
    }
}

/// Coarse reason an OTA check did not complete; logged as `code=`.
///
/// Shared with the task glue in `mod.rs`, which owns the flash/apply stages;
/// the variants the transport never produces are part of that seam.
#[derive(Clone, Copy)]
pub(super) enum OtaError {
    Url,
    Dns,
    Connect,
    Tls,
    Timeout,
    Request,
    Auth,
    Head,
    Version,
    Sha,
    Transport(FetchError),
    Update(&'static str),
}

impl OtaError {
    /// Stable, secret-free identifier for logs.
    pub(super) fn code(&self) -> &'static str {
        match self {
            OtaError::Url => "url",
            OtaError::Dns => "dns",
            OtaError::Connect => "connect",
            OtaError::Tls => "tls",
            OtaError::Timeout => "timeout",
            OtaError::Request => "request",
            OtaError::Auth => "auth",
            OtaError::Head => "head",
            OtaError::Version => "version",
            OtaError::Sha => "sha",
            OtaError::Transport(error) => error.code(),
            OtaError::Update(code) => code,
        }
    }
}

/// Transport-level failure categories. The underlying `Error` types are
/// intentionally dropped: logging them defensively keeps secrets out and lets
/// one error type serve both plain and TLS connections.
#[derive(Clone, Copy)]
pub(super) enum FetchError {
    Timeout,
    Write,
    Flush,
    Read,
    Closed,
    Head(&'static str),
    Leftover,
    BodyTooLarge,
}

impl FetchError {
    /// Stable, secret-free identifier for logs.
    pub(super) fn code(self) -> &'static str {
        match self {
            FetchError::Timeout => "timeout",
            FetchError::Write => "write",
            FetchError::Flush => "flush",
            FetchError::Read => "read",
            FetchError::Closed => "closed",
            FetchError::Head(code) => code,
            FetchError::Leftover => "leftover",
            FetchError::BodyTooLarge => "body_too_large",
        }
    }
}

/// A fixed-capacity `core::fmt::Write` sink for requests and short names.
pub(super) struct TextBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> TextBuf<N> {
    pub(super) fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    pub(super) fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }

    pub(super) fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl<const N: usize> core::fmt::Write for TextBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        if end > N {
            return Err(core::fmt::Error);
        }
        self.buf[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Resolve `uri.host` to an endpoint. IPv4 literals bypass DNS.
pub(super) async fn resolve(stack: Stack<'static>, uri: &Uri<'_>) -> Result<IpEndpoint, OtaError> {
    if let Ok(ip) = uri.host.parse::<Ipv4Addr>() {
        return Ok(IpEndpoint::new(IpAddress::Ipv4(ip), uri.port));
    }
    match with_timeout(DNS_TIMEOUT, crate::net::resolve(stack, uri.host)).await {
        Ok(Some(v4)) => Ok(IpEndpoint::new(IpAddress::Ipv4(v4), uri.port)),
        Ok(None) => Err(OtaError::Dns),
        Err(_) => Err(OtaError::Timeout),
    }
}

/// Open one TCP (and, for `https`, TLS) connection.
// The buffers are four independent slices reborrowed from the task's
// `StaticCell`s; grouping them would add a type without hiding any complexity.
#[allow(clippy::too_many_arguments)]
pub(super) async fn open<'a>(
    stack: Stack<'static>,
    endpoint: IpEndpoint,
    uri: &Uri<'_>,
    rng: esp_hal::rng::Rng,
    tcp_rx: &'a mut [u8],
    tcp_tx: &'a mut [u8],
    rec_read: &'a mut [u8],
    rec_write: &'a mut [u8],
) -> Result<Opened<'a>, OtaError> {
    let mut socket = TcpSocket::new(stack, tcp_rx, tcp_tx);
    match with_timeout(CONNECT_TIMEOUT, socket.connect(endpoint)).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => return Err(OtaError::Connect),
        Err(_) => return Err(OtaError::Timeout),
    }

    if !uri.tls {
        return Ok(Opened::Plain(socket));
    }

    let mut connection = TlsConnection::new(socket, rec_read, rec_write);
    // `TlsConfig::new` already adds the RSA schemes when the `alloc` feature is
    // compiled in, but state it explicitly: the real endpoint presents a
    // Let's Encrypt RSA certificate, and RSA-PSS must be advertised or the
    // handshake fails before any verification runs.
    let config = TlsConfig::new()
        .with_server_name(uri.host)
        .enable_rsa_signatures();
    let provider = NoVerifyProvider {
        rng: EspRng::new(rng),
    };
    match with_timeout(
        HANDSHAKE_TIMEOUT,
        connection.open(TlsContext::new(&config, provider)),
    )
    .await
    {
        Ok(Ok(())) => Ok(Opened::Tls(connection)),
        Ok(Err(_)) => Err(OtaError::Tls),
        Err(_) => Err(OtaError::Timeout),
    }
}

/// An open connection: plain TCP or TLS over it. Only one variant is ever live
/// and there is no allocator to box the larger TLS state with.
#[allow(clippy::large_enum_variant)]
pub(super) enum Opened<'a> {
    Plain(TcpSocket<'a>),
    Tls(TlsConnection<'a, TcpSocket<'a>, Aes128GcmSha256>),
}

impl Opened<'_> {
    /// Send one request and feed bytes to the head parser until it terminates.
    /// On success the returned length is the body bytes already read into
    /// `leftover`.
    pub(super) async fn send(
        &mut self,
        request: &[u8],
        leftover: &mut [u8],
    ) -> Result<(ResponseHead, usize), FetchError> {
        match self {
            Opened::Plain(transport) => get(transport, request, leftover).await,
            Opened::Tls(transport) => get(transport, request, leftover).await,
        }
    }

    /// Read the next slice of the body.
    async fn read_some(&mut self, buf: &mut [u8]) -> Result<usize, FetchError> {
        match self {
            Opened::Plain(transport) => read_timed(transport, buf).await,
            Opened::Tls(transport) => read_timed(transport, buf).await,
        }
    }

    /// Read exactly `len` bytes of a bounded text body, draining the over-read
    /// leftover first. Stops early on EOF.
    pub(super) async fn read_small(
        &mut self,
        leftover: &[u8],
        len: u64,
        out: &mut [u8],
    ) -> Result<usize, FetchError> {
        if len as usize > out.len() {
            return Err(FetchError::BodyTooLarge);
        }
        let len = len as usize;
        let take = core::cmp::min(leftover.len(), len);
        out[..take].copy_from_slice(&leftover[..take]);
        let mut n = take;
        while n < len {
            let read = self.read_some(&mut out[n..len]).await?;
            if read == 0 {
                break;
            }
            n += read;
        }
        Ok(n)
    }
}

/// Write the request, flush, then feed every read byte to the head parser.
async fn get<T: Read + Write>(
    transport: &mut T,
    request: &[u8],
    leftover: &mut [u8],
) -> Result<(ResponseHead, usize), FetchError> {
    with_timeout(IO_TIMEOUT, transport.write_all(request))
        .await
        .map_err(|_| FetchError::Timeout)?
        .map_err(|_| FetchError::Write)?;
    with_timeout(IO_TIMEOUT, transport.flush())
        .await
        .map_err(|_| FetchError::Timeout)?
        .map_err(|_| FetchError::Flush)?;

    let mut parser = HeadParser::new();
    let mut buf = [0u8; READ_CHUNK];
    loop {
        let n = read_timed(transport, &mut buf).await?;
        if n == 0 {
            return Err(FetchError::Closed);
        }
        for (i, &byte) in buf[..n].iter().enumerate() {
            match parser.push(byte) {
                HeadEvent::NeedMore => {}
                HeadEvent::Complete(head) => {
                    let extra = n - i - 1;
                    if extra > leftover.len() {
                        return Err(FetchError::Leftover);
                    }
                    leftover[..extra].copy_from_slice(&buf[i + 1..n]);
                    return Ok((head, extra));
                }
                HeadEvent::Reject(error) => return Err(FetchError::Head(head_code(error))),
            }
        }
    }
}

async fn read_timed<T: Read>(transport: &mut T, buf: &mut [u8]) -> Result<usize, FetchError> {
    with_timeout(IO_TIMEOUT, transport.read(buf))
        .await
        .map_err(|_| FetchError::Timeout)?
        .map_err(|_| FetchError::Read)
}

/// Streaming body source for [`apply_update`](skylights_core::ota::session::apply_update):
/// the already-read leftover first, then the connection.
pub(super) struct HttpBody<'a, 'b> {
    pub(super) link: &'a mut Opened<'b>,
    pub(super) leftover: &'a [u8],
}

impl BodyReader for HttpBody<'_, '_> {
    type Error = FetchError;

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if !self.leftover.is_empty() {
            let n = core::cmp::min(buf.len(), self.leftover.len());
            buf[..n].copy_from_slice(&self.leftover[..n]);
            self.leftover = &self.leftover[n..];
            return Ok(n);
        }
        self.link.read_some(buf).await
    }
}

/// Build `GET {path}/{name} HTTP/1.0` with `Host`, optional Basic auth and
/// `Connection: close`. The port is included in `Host` only when it is not the
/// scheme default.
pub(super) fn build_request(
    uri: &Uri<'_>,
    name: &str,
) -> Result<TextBuf<REQUEST_MAX_BYTES>, OtaError> {
    let mut out = TextBuf::<REQUEST_MAX_BYTES>::new();
    write!(out, "GET {}/{} HTTP/1.0\r\n", uri.path(), name).map_err(|_| OtaError::Request)?;
    let default_port = if uri.tls { 443 } else { 80 };
    if uri.port == default_port {
        write!(out, "Host: {}\r\n", uri.host).map_err(|_| OtaError::Request)?;
    } else {
        write!(out, "Host: {}:{}\r\n", uri.host, uri.port).map_err(|_| OtaError::Request)?;
    }
    if !secrets::OTA_USER.is_empty() || !secrets::OTA_PASSWORD.is_empty() {
        // Never send credentials over an unencrypted transport.
        if !uri.tls {
            return Err(OtaError::Auth);
        }
        let mut auth = [0u8; AUTH_MAX_BYTES];
        let n = basic_authorization(secrets::OTA_USER, secrets::OTA_PASSWORD, &mut auth)
            .ok_or(OtaError::Auth)?;
        let auth = core::str::from_utf8(&auth[..n]).map_err(|_| OtaError::Auth)?;
        write!(out, "Authorization: {}\r\n", auth).map_err(|_| OtaError::Request)?;
    }
    write!(out, "Connection: close\r\n\r\n").map_err(|_| OtaError::Request)?;
    Ok(out)
}

fn head_code(error: HeadError) -> &'static str {
    match error {
        HeadError::MalformedStatusLine => "malformed_status",
        HeadError::TooFewStatusTokens => "too_few_status_tokens",
        HeadError::NonNumericStatus => "non_numeric_status",
        HeadError::UnexpectedStatus => "unexpected_status",
        HeadError::TransferEncoding => "transfer_encoding",
        HeadError::MissingContentLength => "missing_content_length",
        HeadError::DuplicateContentLength => "duplicate_content_length",
        HeadError::MalformedHeader => "malformed_header",
        HeadError::OversizedHeader => "oversized_header",
    }
}
