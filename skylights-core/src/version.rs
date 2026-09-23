//! Firmware version parsing and validation utilities.
//!
//! Provides parsing of single-line integer version strings (e.g. from `VERSION`)
//! into a strongly-typed `u32` value suitable for build injection and OTA compatibility checks.

use core::fmt;

/// Errors that may occur when parsing a version string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionError {
    /// Provided string is empty or contains only whitespace.
    Empty,
    /// String contains invalid characters or format (e.g. non-numeric characters, negative values).
    InvalidFormat,
    /// Parsed integer exceeds the maximum value of a 32-bit unsigned integer (`u32::MAX`).
    Overflow,
}

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "version string is empty"),
            Self::InvalidFormat => write!(f, "version string contains invalid characters"),
            Self::Overflow => write!(f, "version number overflows u32"),
        }
    }
}

/// Parses an unsigned 32-bit integer version from a raw string.
///
/// Trims leading and trailing whitespace and newlines before parsing.
///
/// # Errors
///
/// - [`VersionError::Empty`] if `raw` is empty or only whitespace.
/// - [`VersionError::InvalidFormat`] if `raw` contains non-digit characters (including negative signs).
/// - [`VersionError::Overflow`] if the number exceeds `u32::MAX` (4,294,967,295).
///
/// # Examples
///
/// ```
/// use skylights_core::version::{parse_version, VersionError};
///
/// assert_eq!(parse_version("1\n"), Ok(1));
/// assert_eq!(parse_version("  42  "), Ok(42));
/// assert_eq!(parse_version(""), Err(VersionError::Empty));
/// assert_eq!(parse_version("abc"), Err(VersionError::InvalidFormat));
/// assert_eq!(parse_version("-5"), Err(VersionError::InvalidFormat));
/// assert_eq!(parse_version("4294967296"), Err(VersionError::Overflow));
/// ```
pub fn parse_version(raw: &str) -> Result<u32, VersionError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(VersionError::Empty);
    }

    match trimmed.parse::<u32>() {
        Ok(val) => Ok(val),
        Err(err) => match err.kind() {
            core::num::IntErrorKind::Empty => Err(VersionError::Empty),
            core::num::IntErrorKind::PosOverflow | core::num::IntErrorKind::NegOverflow => {
                Err(VersionError::Overflow)
            }
            _ => Err(VersionError::InvalidFormat),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_valid_integers() {
        assert_eq!(parse_version("0"), Ok(0));
        assert_eq!(parse_version("1"), Ok(1));
        assert_eq!(parse_version("42"), Ok(42));
        assert_eq!(parse_version("100"), Ok(100));
        assert_eq!(parse_version("4294967295"), Ok(u32::MAX));
    }

    #[test]
    fn test_parse_version_whitespace_and_newlines() {
        assert_eq!(parse_version("1\n"), Ok(1));
        assert_eq!(parse_version("1\r\n"), Ok(1));
        assert_eq!(parse_version("  42  "), Ok(42));
        assert_eq!(parse_version("\t\n 123 \n\t"), Ok(123));
        assert_eq!(parse_version("  0  \n"), Ok(0));
    }

    #[test]
    fn test_parse_version_empty_strings() {
        assert_eq!(parse_version(""), Err(VersionError::Empty));
        assert_eq!(parse_version(" "), Err(VersionError::Empty));
        assert_eq!(parse_version("\n"), Err(VersionError::Empty));
        assert_eq!(parse_version("\r\n"), Err(VersionError::Empty));
        assert_eq!(parse_version("\t   \n"), Err(VersionError::Empty));
    }

    #[test]
    fn test_parse_version_invalid_characters() {
        assert_eq!(parse_version("abc"), Err(VersionError::InvalidFormat));
        assert_eq!(parse_version("1.0"), Err(VersionError::InvalidFormat));
        assert_eq!(parse_version("1.0.0"), Err(VersionError::InvalidFormat));
        assert_eq!(parse_version("v1"), Err(VersionError::InvalidFormat));
        assert_eq!(parse_version("1_000"), Err(VersionError::InvalidFormat));
        assert_eq!(parse_version("1 2"), Err(VersionError::InvalidFormat));
        assert_eq!(parse_version("1a"), Err(VersionError::InvalidFormat));
        assert_eq!(parse_version("!@#"), Err(VersionError::InvalidFormat));
    }

    #[test]
    fn test_parse_version_negative_numbers() {
        assert_eq!(parse_version("-1"), Err(VersionError::InvalidFormat));
        assert_eq!(parse_version("-42"), Err(VersionError::InvalidFormat));
        assert_eq!(parse_version(" -100 \n"), Err(VersionError::InvalidFormat));
    }

    #[test]
    fn test_parse_version_overflow() {
        // u32::MAX is 4,294,967,295
        assert_eq!(parse_version("4294967296"), Err(VersionError::Overflow));
        assert_eq!(parse_version("5000000000"), Err(VersionError::Overflow));
        assert_eq!(
            parse_version("99999999999999999999999999"),
            Err(VersionError::Overflow)
        );
    }

    #[test]
    fn test_version_error_display() {
        assert_eq!(
            format!("{}", VersionError::Empty),
            "version string is empty"
        );
        assert_eq!(
            format!("{}", VersionError::InvalidFormat),
            "version string contains invalid characters"
        );
        assert_eq!(
            format!("{}", VersionError::Overflow),
            "version number overflows u32"
        );
    }
}
