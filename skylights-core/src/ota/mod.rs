//! Over-The-Air (OTA) update domain logic.
//!
//! Pure, allocation-free policy shared by host tests and firmware: update
//! decisioning, base-URL parsing, version/hash parsing, HTTP Basic auth, the
//! byte-fed HTTP/1.0 head parser, and the streaming verified update session.
//! Flash-partition geometry and the [`OtaStorage`] abstraction live in the
//! [`partition`] submodule and are re-exported here.

pub mod http;
pub mod partition;
pub mod session;

pub use partition::{
    crc32_ieee, next_seq_for_slot, resolve_active_slot, EspOtaSelectEntry, FlashError,
    MockOtaStorage, OtaStorage, OtadataResolution, OtadataSector, Slot, CRC32_TABLE,
    ESP_OTA_IMG_ABORTED, ESP_OTA_IMG_INVALID, ESP_OTA_IMG_NEW, ESP_OTA_IMG_PENDING_VERIFY,
    ESP_OTA_IMG_UNDEFINED, ESP_OTA_IMG_VALID, FLASH_SECTOR_SIZE, OTADATA_OFFSET, OTADATA_SIZE,
    OTA_0_OFFSET, OTA_1_OFFSET, OTA_SLOT_SIZE,
};

/// Size of one streaming OTA download chunk (4 KiB).
pub const CHUNK_BYTES: usize = 4096;

/// Maximum length of a request path stored by [`Uri`].
const PATH_BYTES: usize = 192;

/// Pure OTA update policy decision.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    /// The remote image differs from the local version; flash it.
    Update,
    /// The remote image matches the local version; do nothing.
    Skip,
}

/// Decides whether to apply the remote image.
///
/// Returns [`Decision::Update`] whenever `remote != local`, so both upgrades
/// and server-side downgrades are permitted.
pub fn decide(local: u32, remote: u32) -> Decision {
    if remote != local {
        Decision::Update
    } else {
        Decision::Skip
    }
}

/// Reasons [`parse_url`] can reject a base URL.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UrlError {
    /// Missing or unsupported scheme (only `http` / `https`).
    Scheme,
    /// Empty or malformed authority host.
    Host,
    /// Malformed or zero port.
    Port,
    /// Empty or malformed project name.
    Project,
    /// Resulting path is overlong or contains an invalid segment.
    Path,
}

/// Parsed, allocation-free OTA base URL.
#[derive(Debug)]
pub struct Uri<'a> {
    /// True for `https`, false for `http`.
    pub tls: bool,
    /// Authority host.
    pub host: &'a str,
    /// Explicit or default port.
    pub port: u16,
    /// Fixed-capacity request path buffer.
    path: [u8; PATH_BYTES],
    /// Number of valid bytes in `path`.
    path_len: usize,
}

impl Uri<'_> {
    /// Returns the request path, always prefixed with `/`.
    pub fn path(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len]).unwrap_or("")
    }
}

/// Parses a base URL and appends `/{project}` to any existing path prefix.
///
/// `https` defaults to port 443, `http` to port 80. Rejects empty host/project,
/// malformed or zero ports, and paths longer than [`PATH_BYTES`].
pub fn parse_url<'a>(base: &'a str, project: &'a str) -> Result<Uri<'a>, UrlError> {
    let (scheme, rest) = base.split_once("://").ok_or(UrlError::Scheme)?;
    let tls = match scheme {
        "https" => true,
        "http" => false,
        _ => return Err(UrlError::Scheme),
    };
    if !valid_segment(project) {
        return Err(UrlError::Project);
    }
    let (authority, prefix) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (host, port) = parse_authority(authority, tls)?;

    let mut path = [0u8; PATH_BYTES];
    let mut len = 0;
    for segment in prefix.split('/').filter(|s| !s.is_empty()).chain([project]) {
        if len + 1 + segment.len() > PATH_BYTES || !valid_segment(segment) {
            return Err(UrlError::Path);
        }
        path[len] = b'/';
        len += 1;
        path[len..len + segment.len()].copy_from_slice(segment.as_bytes());
        len += segment.len();
    }
    Ok(Uri {
        tls,
        host,
        port,
        path,
        path_len: len,
    })
}

/// Splits an authority into a host and port, applying the scheme default.
fn parse_authority(authority: &str, tls: bool) -> Result<(&str, u16), UrlError> {
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port = port.parse::<u16>().map_err(|_| UrlError::Port)?;
            if port == 0 {
                return Err(UrlError::Port);
            }
            (host, port)
        }
        None => (authority, if tls { 443 } else { 80 }),
    };
    if host.is_empty() || !host.bytes().all(valid_host_byte) {
        return Err(UrlError::Host);
    }
    Ok((host, port))
}

/// Accepts the byte subset used for host and path segments.
fn valid_host_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_'
}

/// A path/host segment must be non-empty, must not be a `.`/`..` traversal
/// token, and must use only [`valid_host_byte`]s.
fn valid_segment(segment: &str) -> bool {
    !segment.is_empty() && segment != "." && segment != ".." && segment.bytes().all(valid_host_byte)
}

/// Parses a decimal version number, trimming ASCII whitespace.
///
/// Returns `None` for empty, non-numeric, or overflowing input.
pub fn parse_version(body: &[u8]) -> Option<u32> {
    let s = trim_ascii(body);
    if s.is_empty() || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut value: u32 = 0;
    for &b in s {
        value = value.checked_mul(10)?.checked_add(u32::from(b - b'0'))?;
    }
    Some(value)
}

/// Parses exactly 64 lowercase hex characters into a 32-byte digest.
pub fn parse_sha256_hex(body: &[u8]) -> Option<[u8; 32]> {
    let s = trim_ascii(body);
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let hi = hex_nibble(s[2 * i])?;
        let lo = hex_nibble(s[2 * i + 1])?;
        *byte = (hi << 4) | lo;
    }
    Some(out)
}

/// Decodes one lowercase hex digit.
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// Standard-alphabet base64 encoder with `=` padding.
///
/// Returns the number of bytes written or `None` if `out` is too small.
pub fn base64(input: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut bytes = input.iter().copied();
    base64_fill(|| bytes.next(), out)
}

/// Writes `Basic <base64(user:pass)>` into `out`.
///
/// Returns `None` when both `user` and `pass` are empty, or when `out` is too small.
pub fn basic_authorization(user: &str, pass: &str, out: &mut [u8]) -> Option<usize> {
    const PREFIX: &[u8] = b"Basic ";
    if user.is_empty() && pass.is_empty() {
        return None;
    }
    if out.len() < PREFIX.len() {
        return None;
    }
    out[..PREFIX.len()].copy_from_slice(PREFIX);
    let mut bytes = user
        .bytes()
        .chain(core::iter::once(b':'))
        .chain(pass.bytes());
    let n = base64_fill(|| bytes.next(), &mut out[PREFIX.len()..])?;
    Some(PREFIX.len() + n)
}

/// Standard base64 alphabet `A-Za-z0-9+/`.
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes 3-byte groups, then pads the final partial group with `=`.
fn base64_fill<F: FnMut() -> Option<u8>>(mut next: F, out: &mut [u8]) -> Option<usize> {
    let mut o = 0;
    loop {
        let b0 = match next() {
            Some(byte) => byte,
            None => return Some(o),
        };
        let b1 = next();
        let b2 = next();
        if out.len() < o + 4 {
            return None;
        }
        let n =
            (u32::from(b0) << 16) | (u32::from(b1.unwrap_or(0)) << 8) | u32::from(b2.unwrap_or(0));
        out[o] = B64[((n >> 18) & 63) as usize];
        out[o + 1] = B64[((n >> 12) & 63) as usize];
        out[o + 2] = match b1 {
            Some(_) => B64[((n >> 6) & 63) as usize],
            None => b'=',
        };
        out[o + 3] = match b2 {
            Some(_) => B64[(n & 63) as usize],
            None => b'=',
        };
        o += 4;
        if b2.is_none() {
            return Some(o);
        }
    }
}

/// Strips leading and trailing ASCII whitespace from a byte slice.
pub(crate) fn trim_ascii(input: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = input.len();
    while start < end && input[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && input[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    &input[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_decide_updates_on_any_difference() {
        assert_eq!(decide(5, 5), Decision::Skip);
        assert_eq!(decide(0, 0), Decision::Skip);
        assert_eq!(decide(5, 6), Decision::Update);
        assert_eq!(decide(6, 5), Decision::Update);
        assert_eq!(decide(0, u32::MAX), Decision::Update);
    }

    #[test]
    fn policy_parse_url_accepts_schemes_ports_and_prefixes() {
        let https = parse_url("https://ota.example.com", "proj").unwrap();
        assert!(https.tls);
        assert_eq!(https.host, "ota.example.com");
        assert_eq!(https.port, 443);
        assert_eq!(https.path(), "/proj");

        let http = parse_url("http://ota.example.com", "proj").unwrap();
        assert!(!http.tls);
        assert_eq!(http.port, 80);
        assert_eq!(http.path(), "/proj");

        assert_eq!(
            parse_url("https://ota.example.com:8443", "proj")
                .unwrap()
                .port,
            8443
        );
        assert_eq!(
            parse_url("http://ota.example.com:8080", "proj")
                .unwrap()
                .port,
            8080
        );

        assert_eq!(
            parse_url("https://ota.example.com/base/", "proj")
                .unwrap()
                .path(),
            "/base/proj"
        );
        assert_eq!(
            parse_url("https://ota.example.com/a/b", "proj")
                .unwrap()
                .path(),
            "/a/b/proj"
        );
        assert_eq!(
            parse_url("https://ota.example.com/", "proj")
                .unwrap()
                .path(),
            "/proj"
        );
    }

    #[test]
    fn policy_parse_url_rejects_each_error() {
        assert!(matches!(
            parse_url("ota.example.com", "proj"),
            Err(UrlError::Scheme)
        ));
        assert!(matches!(
            parse_url("ftp://ota.example.com", "proj"),
            Err(UrlError::Scheme)
        ));

        assert!(matches!(parse_url("https://", "proj"), Err(UrlError::Host)));
        assert!(matches!(
            parse_url("https://:443", "proj"),
            Err(UrlError::Host)
        ));
        assert!(matches!(
            parse_url("https://bad host", "proj"),
            Err(UrlError::Host)
        ));

        assert!(matches!(
            parse_url("https://ota.example.com:0", "proj"),
            Err(UrlError::Port)
        ));
        assert!(matches!(
            parse_url("https://ota.example.com:abc", "proj"),
            Err(UrlError::Port)
        ));
        assert!(matches!(
            parse_url("https://ota.example.com:70000", "proj"),
            Err(UrlError::Port)
        ));

        assert!(matches!(
            parse_url("https://ota.example.com", ""),
            Err(UrlError::Project)
        ));
        assert!(matches!(
            parse_url("https://ota.example.com", "bad/proj"),
            Err(UrlError::Project)
        ));

        // `.`/`..` traversal tokens are rejected in the project and path segments.
        assert!(matches!(
            parse_url("https://ota.example.com", "."),
            Err(UrlError::Project)
        ));
        assert!(matches!(
            parse_url("https://ota.example.com", ".."),
            Err(UrlError::Project)
        ));
        assert!(matches!(
            parse_url("https://ota.example.com/../x", "proj"),
            Err(UrlError::Path)
        ));

        assert!(matches!(
            parse_url("https://ota.example.com/bad%20seg", "proj"),
            Err(UrlError::Path)
        ));

        let mut buf = [b'a'; 260];
        let prefix = b"https://ota.example.com/";
        buf[..prefix.len()].copy_from_slice(prefix);
        let base = core::str::from_utf8(&buf).unwrap();
        assert!(matches!(parse_url(base, "proj"), Err(UrlError::Path)));
    }

    #[test]
    fn policy_parse_version_parses_and_trims() {
        assert_eq!(parse_version(b"42"), Some(42));
        assert_eq!(parse_version(b"  42\n"), Some(42));
        assert_eq!(parse_version(b"\t7 \r\n"), Some(7));
        assert_eq!(parse_version(b"0"), Some(0));
        assert_eq!(parse_version(b"4294967295"), Some(u32::MAX));
    }

    #[test]
    fn policy_parse_version_rejects_invalid_and_overflow() {
        assert_eq!(parse_version(b""), None);
        assert_eq!(parse_version(b"   "), None);
        assert_eq!(parse_version(b"12a"), None);
        assert_eq!(parse_version(b"-1"), None);
        assert_eq!(parse_version(b"1 2"), None);
        assert_eq!(parse_version(b"4294967296"), None);
        assert_eq!(parse_version(b"99999999999999999999"), None);
    }

    #[test]
    fn policy_parse_sha256_hex_parses_and_trims() {
        let zeros = b"0000000000000000000000000000000000000000000000000000000000000000";
        assert_eq!(parse_sha256_hex(zeros), Some([0u8; 32]));

        let hex = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let digest = parse_sha256_hex(hex).unwrap();
        assert_eq!(digest[0], 0x01);
        assert_eq!(digest[1], 0x23);
        assert_eq!(digest[31], 0xef);

        let trimmed = b"\n  0000000000000000000000000000000000000000000000000000000000000000\t";
        assert_eq!(parse_sha256_hex(trimmed), Some([0u8; 32]));
    }

    #[test]
    fn policy_parse_sha256_hex_rejects_invalid() {
        let short = [b'0'; 63];
        assert_eq!(parse_sha256_hex(&short), None);

        let long = [b'0'; 65];
        assert_eq!(parse_sha256_hex(&long), None);

        let upper = [b'A'; 64];
        assert_eq!(parse_sha256_hex(&upper), None);

        let mut non_hex = [b'0'; 64];
        non_hex[10] = b'g';
        assert_eq!(parse_sha256_hex(&non_hex), None);
    }

    #[test]
    fn policy_base64_known_vectors() {
        let cases: [(&[u8], &[u8]); 7] = [
            (b"", b""),
            (b"f", b"Zg=="),
            (b"fo", b"Zm8="),
            (b"foo", b"Zm9v"),
            (b"foob", b"Zm9vYg=="),
            (b"fooba", b"Zm9vYmE="),
            (b"foobar", b"Zm9vYmFy"),
        ];
        for (input, expected) in cases {
            let mut buf = [0u8; 32];
            let n = base64(input, &mut buf).unwrap();
            assert_eq!(&buf[..n], expected);
        }
    }

    #[test]
    fn policy_base64_rejects_small_buffer() {
        let mut small = [0u8; 3];
        assert_eq!(base64(b"foobar", &mut small), None);

        let mut empty: [u8; 0] = [];
        assert_eq!(base64(b"f", &mut empty), None);
        assert_eq!(base64(b"", &mut empty), Some(0));
    }

    #[test]
    fn policy_basic_authorization_formats_header() {
        let mut buf = [0u8; 64];
        let n = basic_authorization("user", "pass", &mut buf).unwrap();
        assert_eq!(&buf[..n], b"Basic dXNlcjpwYXNz");

        let n = basic_authorization("user", "", &mut buf).unwrap();
        assert_eq!(&buf[..n], b"Basic dXNlcjo=");

        assert_eq!(basic_authorization("", "", &mut buf), None);

        let mut small = [0u8; 4];
        assert_eq!(basic_authorization("user", "pass", &mut small), None);
    }
}
