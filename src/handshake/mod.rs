pub mod client;
pub mod server;
#[cfg(feature="tls")]
pub mod tls;

use std::ascii::AsciiExt;
use std::str::from_utf8;

use base64;
use bytes::Buf;
use httparse;
use httparse::Status;
use sha1::Sha1;

use error::Result;

// Limit the number of header lines.
const MAX_HEADERS: usize = 124;

/// A handshake state.
pub trait Handshake: Sized {
    /// Resulting stream of this handshake.
    type Stream;
    /// Perform a single handshake round.
    fn handshake(self) -> Result<HandshakeResult<Self>>;
    /// Perform handshake to the end in a blocking mode.
    fn handshake_wait(self) -> Result<Self::Stream> {
        let mut hs = self;
        loop {
            hs = match hs.handshake()? {
                HandshakeResult::Done(stream) => return Ok(stream),
                HandshakeResult::Incomplete(s) => s,
            }
        }
    }
}

/// A handshake result.
pub enum HandshakeResult<H: Handshake> {
    /// Handshake is done, a WebSocket stream is ready.
    Done(H::Stream),
    /// Handshake is not done, call handshake() again.
    Incomplete(H),
}

impl<H: Handshake> HandshakeResult<H> {
    pub fn map<R, F>(self, func: F) -> HandshakeResult<R>
    where R: Handshake<Stream = H::Stream>,
          F: FnOnce(H) -> R,
    {
        match self {
            HandshakeResult::Done(s) => HandshakeResult::Done(s),
            HandshakeResult::Incomplete(h) => HandshakeResult::Incomplete(func(h)),
        }
    }
}

/// Turns a Sec-WebSocket-Key into a Sec-WebSocket-Accept.
fn convert_key(input: &[u8]) -> Result<String> {
    // ... field is constructed by concatenating /key/ ...
    // ... with the string "258EAFA5-E914-47DA-95CA-C5AB0DC85B11" (RFC 6455)
    const WS_GUID: &'static [u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut sha1 = Sha1::new();
    sha1.update(input);
    sha1.update(WS_GUID);
    Ok(base64::encode(&sha1.digest().bytes()))
}

/// HTTP request or response headers.
#[derive(Debug)]
pub struct Headers {
    data: Vec<(String, Box<[u8]>)>,
}

impl Headers {

    /// Get first header with the given name, if any.
    pub fn find_first(&self, name: &str) -> Option<&[u8]> {
        self.data.iter()
                .find(|&&(ref n, _)| n.eq_ignore_ascii_case(name))
                .map(|&(_, ref v)| v.as_ref())
    }

    /// Check if the given header has the given value.
    pub fn header_is(&self, name: &str, value: &str) -> bool {
        self.find_first(name)
            .map(|v| v == value.as_bytes())
            .unwrap_or(false)
    }

    /// Check if the given header has the given value (case-insensitive).
    pub fn header_is_ignore_case(&self, name: &str, value: &str) -> bool {
        self.find_first(name).ok_or(())
            .and_then(|val_raw| from_utf8(val_raw).map_err(|_| ()))
            .map(|val| val.eq_ignore_ascii_case(value))
            .unwrap_or(false)
    }

    /// Try to parse data and return headers, if any.
    fn parse<B: Buf>(input: &mut B) -> Result<Option<Headers>> {
        Headers::parse_http(input)
    }

}

/// Trait to read HTTP parseable objects.
trait Httparse: Sized {
    fn httparse(buf: &[u8]) -> Result<Option<(usize, Self)>>;
    fn parse_http<B: Buf>(input: &mut B) -> Result<Option<Self>> {
        Ok(match Self::httparse(input.bytes())? {
            Some((size, obj)) => {
                input.advance(size);
                Some(obj)
            },
            None => None,
        })
    }
}

/// Trait to convert raw objects into HTTP parseables.
trait FromHttparse<T>: Sized {
    fn from_httparse(raw: T) -> Result<Self>;
}

impl Httparse for Headers {
    fn httparse(buf: &[u8]) -> Result<Option<(usize, Self)>> {
        let mut hbuffer = [httparse::EMPTY_HEADER; MAX_HEADERS];
        Ok(match httparse::parse_headers(buf, &mut hbuffer)? {
            Status::Partial => None,
            Status::Complete((size, hdr)) => Some((size, Headers::from_httparse(hdr)?)),
        })
    }
}

impl<'b: 'h, 'h> FromHttparse<&'b [httparse::Header<'h>]> for Headers {
    fn from_httparse(raw: &'b [httparse::Header<'h>]) -> Result<Self> {
        Ok(Headers {
            data: raw.iter()
                     .map(|h| (h.name.into(), Vec::from(h.value).into_boxed_slice()))
                     .collect(),
        })
    }
}

#[cfg(test)]
mod tests {

    use super::{Headers, convert_key};

    use std::io::Cursor;

    #[test]
    fn key_conversion() {
        // example from RFC 6455
        assert_eq!(convert_key(b"dGhlIHNhbXBsZSBub25jZQ==").unwrap(),
                               "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn headers() {
        const data: &'static [u8] =
            b"Host: foo.com\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n";
        let mut inp = Cursor::new(data);
        let hdr = Headers::parse(&mut inp).unwrap().unwrap();
        assert_eq!(hdr.find_first("Host"), Some(&b"foo.com"[..]));
        assert_eq!(hdr.find_first("Upgrade"), Some(&b"websocket"[..]));
        assert_eq!(hdr.find_first("Connection"), Some(&b"Upgrade"[..]));

        assert!(hdr.header_is("upgrade", "websocket"));
        assert!(!hdr.header_is("upgrade", "Websocket"));
        assert!(hdr.header_is_ignore_case("upgrade", "Websocket"));
    }

    #[test]
    fn headers_incomplete() {
        const data: &'static [u8] =
            b"Host: foo.com\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n";
        let mut inp = Cursor::new(data);
        let hdr = Headers::parse(&mut inp).unwrap();
        assert!(hdr.is_none());
    }

}
