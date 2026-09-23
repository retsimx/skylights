//! Byte-fed HTTP/1.0 response-head parser for the OTA GET requests.
//!
//! Pure and allocation-free. Bytes are pushed one at a time; the status line and
//! headers are validated as they arrive and the parser terminates on the blank
//! line or the first rejection. It rejects `Transfer-Encoding` (chunked), a
//! missing or duplicate `Content-Length`, non-2xx/404 statuses, malformed lines
//! and an oversized header block. Any bytes read past the terminating CRLF CRLF
//! are body and are reported to the caller via [`HeadParser::consumed`].

use super::trim_ascii;

const MAX_HEAD_BYTES: usize = 2048;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ResponseHead {
    pub status: u16,
    pub content_length: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeadError {
    MalformedStatusLine,
    TooFewStatusTokens,
    NonNumericStatus,
    UnexpectedStatus,
    TransferEncoding,
    MissingContentLength,
    DuplicateContentLength,
    MalformedHeader,
    OversizedHeader,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeadEvent {
    NeedMore,
    Complete(ResponseHead),
    Reject(HeadError),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Status,
    Headers,
    Done,
}

enum Action {
    Continue,
    Status(u16),
    Header(HeaderUpdate),
    Finish,
    Fail(HeadError),
}

struct HeaderUpdate {
    transfer_encoding: bool,
    content_length: Option<u64>,
}

pub struct HeadParser {
    state: State,
    terminal: Option<HeadEvent>,
    line: [u8; MAX_HEAD_BYTES],
    line_len: usize,
    total: usize,
    status: u16,
    content_length: Option<u64>,
    transfer_encoding: bool,
}

impl HeadParser {
    pub fn new() -> Self {
        Self {
            state: State::Status,
            terminal: None,
            line: [0; MAX_HEAD_BYTES],
            line_len: 0,
            total: 0,
            status: 0,
            content_length: None,
            transfer_encoding: false,
        }
    }

    /// Bytes consumed up to and including the terminating CRLF CRLF. Any bytes
    /// after that are body and must not be fed to `push`.
    pub fn consumed(&self) -> usize {
        self.total
    }

    pub fn push(&mut self, b: u8) -> HeadEvent {
        if let Some(event) = self.terminal {
            return event;
        }
        self.total += 1;
        if self.total > MAX_HEAD_BYTES {
            return self.reject(HeadError::OversizedHeader);
        }
        if b != b'\n' {
            if self.line_len >= self.line.len() {
                return self.reject(HeadError::MalformedHeader);
            }
            self.line[self.line_len] = b;
            self.line_len += 1;
            return HeadEvent::NeedMore;
        }

        let mut end = self.line_len;
        if end > 0 && self.line[end - 1] == b'\r' {
            end -= 1;
        }
        let action = {
            let line = &self.line[..end];
            match self.state {
                State::Status => match Self::parse_status(line) {
                    Ok(status) => Action::Status(status),
                    Err(error) => Action::Fail(error),
                },
                State::Headers if line.is_empty() => Action::Finish,
                State::Headers => match Self::parse_header(line) {
                    Ok(update) => Action::Header(update),
                    Err(error) => Action::Fail(error),
                },
                State::Done => Action::Continue,
            }
        };
        self.line_len = 0;
        match action {
            Action::Continue => HeadEvent::NeedMore,
            Action::Fail(error) => self.reject(error),
            Action::Status(status) => {
                self.status = status;
                self.state = State::Headers;
                HeadEvent::NeedMore
            }
            Action::Header(update) => {
                if update.transfer_encoding {
                    self.transfer_encoding = true;
                }
                if let Some(length) = update.content_length {
                    if self.content_length.is_some() {
                        return self.reject(HeadError::DuplicateContentLength);
                    }
                    self.content_length = Some(length);
                }
                HeadEvent::NeedMore
            }
            Action::Finish => self.finish(),
        }
    }

    fn parse_status(line: &[u8]) -> Result<u16, HeadError> {
        if !(line.starts_with(b"HTTP/1.0 ") || line.starts_with(b"HTTP/1.1 ")) {
            return Err(HeadError::MalformedStatusLine);
        }
        let token_count = line.split(|&b| b == b' ').filter(|s| !s.is_empty()).count();
        if token_count < 3 {
            return Err(HeadError::TooFewStatusTokens);
        }
        let rest = &line[9..];
        let code = match rest.iter().position(|&b| b == b' ') {
            Some(i) => &rest[..i],
            None => rest,
        };
        if code.is_empty() || !code.iter().all(u8::is_ascii_digit) {
            return Err(HeadError::NonNumericStatus);
        }
        let mut value: u32 = 0;
        for &d in code {
            value = value * 10 + u32::from(d - b'0');
            if value > u32::from(u16::MAX) {
                return Err(HeadError::NonNumericStatus);
            }
        }
        Ok(value as u16)
    }

    fn parse_header(line: &[u8]) -> Result<HeaderUpdate, HeadError> {
        let colon = line
            .iter()
            .position(|&b| b == b':')
            .ok_or(HeadError::MalformedHeader)?;
        let name = trim_ascii(&line[..colon]);
        let value = trim_ascii(&line[colon + 1..]);
        let mut update = HeaderUpdate {
            transfer_encoding: false,
            content_length: None,
        };
        if name.eq_ignore_ascii_case(b"transfer-encoding") {
            update.transfer_encoding = true;
        } else if name.eq_ignore_ascii_case(b"content-length") {
            if value.is_empty() {
                return Err(HeadError::MalformedHeader);
            }
            let mut parsed: u64 = 0;
            for &d in value {
                if !d.is_ascii_digit() {
                    return Err(HeadError::MalformedHeader);
                }
                parsed = parsed
                    .checked_mul(10)
                    .and_then(|v| v.checked_add(u64::from(d - b'0')))
                    .ok_or(HeadError::MalformedHeader)?;
            }
            update.content_length = Some(parsed);
        }
        Ok(update)
    }

    fn finish(&mut self) -> HeadEvent {
        if self.transfer_encoding {
            return self.reject(HeadError::TransferEncoding);
        }
        if !(200..=299).contains(&self.status) && self.status != 404 {
            return self.reject(HeadError::UnexpectedStatus);
        }
        match self.content_length {
            Some(length) => self.complete(Some(length)),
            None if self.status == 404 => self.complete(None),
            None => self.reject(HeadError::MissingContentLength),
        }
    }

    fn complete(&mut self, content_length: Option<u64>) -> HeadEvent {
        let event = HeadEvent::Complete(ResponseHead {
            status: self.status,
            content_length,
        });
        self.state = State::Done;
        self.terminal = Some(event);
        event
    }

    fn reject(&mut self, error: HeadError) -> HeadEvent {
        let event = HeadEvent::Reject(error);
        self.state = State::Done;
        self.terminal = Some(event);
        event
    }
}

impl Default for HeadParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds every byte until the parser yields a terminal event.
    fn feed(parser: &mut HeadParser, input: &[u8]) -> HeadEvent {
        let mut event = HeadEvent::NeedMore;
        for &b in input {
            event = parser.push(b);
            if !matches!(event, HeadEvent::NeedMore) {
                return event;
            }
        }
        event
    }

    #[test]
    fn completes_200_with_content_length_and_reports_head_bytes() {
        let head = b"HTTP/1.0 200 OK\r\nContent-Length: 5\r\n\r\n";
        let mut parser = HeadParser::new();
        assert_eq!(
            feed(&mut parser, head),
            HeadEvent::Complete(ResponseHead {
                status: 200,
                content_length: Some(5),
            })
        );
        assert_eq!(parser.consumed(), head.len());

        // Body bytes are not counted: `total - consumed()` identifies them.
        let body = b"hello";
        let pushed = head.len() + body.len();
        assert_eq!(
            feed(&mut parser, body),
            HeadEvent::Complete(ResponseHead {
                status: 200,
                content_length: Some(5),
            })
        );
        assert_eq!(parser.consumed(), head.len());
        assert_eq!(pushed - parser.consumed(), body.len());
    }

    #[test]
    fn accepts_http_1_1_and_204_with_content_length() {
        let mut parser = HeadParser::new();
        assert_eq!(
            feed(
                &mut parser,
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n",
            ),
            HeadEvent::Complete(ResponseHead {
                status: 204,
                content_length: Some(0),
            })
        );
    }

    #[test]
    fn rejects_missing_content_length() {
        let mut parser = HeadParser::new();
        assert_eq!(
            feed(&mut parser, b"HTTP/1.0 200 OK\r\nServer: x\r\n\r\n"),
            HeadEvent::Reject(HeadError::MissingContentLength)
        );
    }

    #[test]
    fn rejects_duplicate_content_length() {
        let mut parser = HeadParser::new();
        assert_eq!(
            feed(
                &mut parser,
                b"HTTP/1.0 200 OK\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\n",
            ),
            HeadEvent::Reject(HeadError::DuplicateContentLength)
        );
    }

    #[test]
    fn rejects_transfer_encoding_chunked() {
        let mut parser = HeadParser::new();
        assert_eq!(
            feed(
                &mut parser,
                b"HTTP/1.0 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 5\r\n\r\n",
            ),
            HeadEvent::Reject(HeadError::TransferEncoding)
        );
    }

    #[test]
    fn accepts_404_without_content_length() {
        let mut parser = HeadParser::new();
        assert_eq!(
            feed(&mut parser, b"HTTP/1.0 404 Not Found\r\n\r\n"),
            HeadEvent::Complete(ResponseHead {
                status: 404,
                content_length: None,
            })
        );
    }

    #[test]
    fn accepts_404_with_content_length() {
        let mut parser = HeadParser::new();
        assert_eq!(
            feed(
                &mut parser,
                b"HTTP/1.0 404 Not Found\r\nContent-Length: 0\r\n\r\n",
            ),
            HeadEvent::Complete(ResponseHead {
                status: 404,
                content_length: Some(0),
            })
        );
    }

    #[test]
    fn rejects_unexpected_status() {
        let mut parser = HeadParser::new();
        assert_eq!(
            feed(
                &mut parser,
                b"HTTP/1.0 500 Server Error\r\nContent-Length: 0\r\n\r\n",
            ),
            HeadEvent::Reject(HeadError::UnexpectedStatus)
        );

        let mut parser = HeadParser::new();
        assert_eq!(
            feed(
                &mut parser,
                b"HTTP/1.0 301 Moved\r\nContent-Length: 0\r\n\r\n",
            ),
            HeadEvent::Reject(HeadError::UnexpectedStatus)
        );
    }

    #[test]
    fn rejects_malformed_status_line() {
        let mut parser = HeadParser::new();
        assert_eq!(
            feed(&mut parser, b"HTTP/2.0 200 OK\r\n\r\n"),
            HeadEvent::Reject(HeadError::MalformedStatusLine)
        );

        let mut parser = HeadParser::new();
        assert_eq!(
            feed(&mut parser, b"GET / HTTP/1.0\r\n\r\n"),
            HeadEvent::Reject(HeadError::MalformedStatusLine)
        );
    }

    #[test]
    fn rejects_too_few_status_tokens_and_non_numeric_status() {
        let mut parser = HeadParser::new();
        assert_eq!(
            feed(&mut parser, b"HTTP/1.0 200\r\n\r\n"),
            HeadEvent::Reject(HeadError::TooFewStatusTokens)
        );

        let mut parser = HeadParser::new();
        assert_eq!(
            feed(&mut parser, b"HTTP/1.0 abc OK\r\n\r\n"),
            HeadEvent::Reject(HeadError::NonNumericStatus)
        );
    }

    #[test]
    fn rejects_malformed_header_without_colon() {
        let mut parser = HeadParser::new();
        assert_eq!(
            feed(&mut parser, b"HTTP/1.0 200 OK\r\nContent-Length 5\r\n\r\n",),
            HeadEvent::Reject(HeadError::MalformedHeader)
        );
    }

    #[test]
    fn rejects_malformed_content_length_value() {
        let mut parser = HeadParser::new();
        assert_eq!(
            feed(
                &mut parser,
                b"HTTP/1.0 200 OK\r\nContent-Length: 5x\r\n\r\n"
            ),
            HeadEvent::Reject(HeadError::MalformedHeader)
        );

        let mut parser = HeadParser::new();
        assert_eq!(
            feed(&mut parser, b"HTTP/1.0 200 OK\r\nContent-Length:\r\n\r\n"),
            HeadEvent::Reject(HeadError::MalformedHeader)
        );
    }

    #[test]
    fn rejects_oversized_header_block() {
        let mut parser = HeadParser::new();
        let mut input = Vec::new();
        input.extend_from_slice(b"HTTP/1.0 200 OK\r\n");
        while input.len() < MAX_HEAD_BYTES + 16 {
            input.extend_from_slice(b"X-Pad: 0123456789\r\n");
        }
        assert_eq!(
            feed(&mut parser, &input),
            HeadEvent::Reject(HeadError::OversizedHeader)
        );
        assert!(parser.consumed() > MAX_HEAD_BYTES);
    }

    #[test]
    fn header_names_are_case_insensitive_and_unknown_headers_ignored() {
        let mut parser = HeadParser::new();
        assert_eq!(
            feed(
                &mut parser,
                b"HTTP/1.0 200 OK\r\ncOnTeNt-LeNgTh: 42\r\nX-Unknown: whatever\r\n\r\n",
            ),
            HeadEvent::Complete(ResponseHead {
                status: 200,
                content_length: Some(42),
            })
        );

        let mut parser = HeadParser::new();
        assert_eq!(
            feed(
                &mut parser,
                b"HTTP/1.0 200 OK\r\nContent-Length: 7\r\nTRANSFER-ENCODING: identity\r\n\r\n",
            ),
            HeadEvent::Reject(HeadError::TransferEncoding)
        );
    }
}
