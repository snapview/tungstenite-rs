use std::io::{Cursor, Read, Write};
use bytes::Buf;
use httparse;
use httparse::Status;

use input_buffer::{InputBuffer, MIN_READ};
use error::{Error, Result};
use protocol::{WebSocket, Role};
use super::{
    Handshake,
    HandshakeResult,
    Headers,
    Httparse,
    FromHttparse,
    convert_key,
    MAX_HEADERS
};
use util::NonBlockingResult;

/// Request from the client.
pub struct Request {
    path: String,
    headers: Headers,
}

impl Request {
    /// Parse the request from a stream.
    pub fn parse<B: Buf>(input: &mut B) -> Result<Option<Self>> {
        Request::parse_http(input)
    }
    /// Reply to the response.
    pub fn reply(&self) -> Result<Vec<u8>> {
        let key = self.headers.find_first("Sec-WebSocket-Key")
            .ok_or(Error::Protocol("Missing Sec-WebSocket-Key".into()))?;
        let reply = format!("\
        HTTP/1.1 101 Switching Protocols\r\n\
        Connection: Upgrade\r\n\
        Upgrade: websocket\r\n\
        Sec-WebSocket-Accept: {}\r\n\
        \r\n", convert_key(key)?);
        Ok(reply.into())
    }
}

impl Httparse for Request {
    fn httparse(buf: &[u8]) -> Result<Option<(usize, Self)>> {
        let mut hbuffer = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut req = httparse::Request::new(&mut hbuffer);
        Ok(match req.parse(buf)? {
            Status::Partial => None,
            Status::Complete(size) => Some((size, Request::from_httparse(req)?)),
        })
    }
}

impl<'h, 'b: 'h> FromHttparse<httparse::Request<'h, 'b>> for Request {
    fn from_httparse(raw: httparse::Request<'h, 'b>) -> Result<Self> {
        if raw.method.expect("Bug: no method in header") != "GET" {
            return Err(Error::Protocol("Method is not GET".into()));
        }
        if raw.version.expect("Bug: no HTTP version") < /*1.*/1 {
            return Err(Error::Protocol("HTTP version should be 1.1 or higher".into()));
        }
        Ok(Request {
            path: raw.path.expect("Bug: no path in header").into(),
            headers: Headers::from_httparse(raw.headers)?
        })
    }
}

/// Server handshake
pub struct ServerHandshake<Stream> {
    stream: Stream,
    state: HandshakeState,
}

impl<Stream: Read + Write> ServerHandshake<Stream> {
    /// Start a new server handshake on top of given stream.
    pub fn new(stream: Stream) -> Self {
        ServerHandshake {
            stream: stream,
            state: HandshakeState::ReceivingRequest(InputBuffer::with_capacity(MIN_READ)),
        }
    }
}

impl<Stream: Read + Write> Handshake for ServerHandshake<Stream> {
    type Stream = WebSocket<Stream>;
    fn handshake(mut self) -> Result<HandshakeResult<Self>> {
        debug!("Performing server handshake...");
        match self.state {
            HandshakeState::ReceivingRequest(mut req_buf) => {
                req_buf.reserve(MIN_READ, usize::max_value())
                    .map_err(|_| Error::Capacity("Header too long".into()))?;
                req_buf.read_from(&mut self.stream).no_block()?;
                let state = if let Some(req) = Request::parse(&mut req_buf)? {
                    let resp = req.reply()?;
                    HandshakeState::SendingResponse(Cursor::new(resp))
                } else {
                    HandshakeState::ReceivingRequest(req_buf)
                };
                Ok(HandshakeResult::Incomplete(ServerHandshake {
                    state: state,
                    ..self
                }))
            }
            HandshakeState::SendingResponse(mut resp) => {
                let size = self.stream.write(Buf::bytes(&resp)).no_block()?.unwrap_or(0);
                Buf::advance(&mut resp, size);
                if resp.has_remaining() {
                    Ok(HandshakeResult::Incomplete(ServerHandshake {
                        state: HandshakeState::SendingResponse(resp),
                        ..self
                    }))
                } else {
                    let ws = WebSocket::from_raw_socket(self.stream, Role::Server);
                    Ok(HandshakeResult::Done(ws))
                }
            }
        }
    }
}

enum HandshakeState {
    ReceivingRequest(InputBuffer),
    SendingResponse(Cursor<Vec<u8>>),
}

#[cfg(test)]
mod tests {

    use super::Request;

    use std::io::Cursor;

    #[test]
    fn request_parsing() {
        const data: &'static [u8] = b"GET /script.ws HTTP/1.1\r\nHost: foo.com\r\n\r\n";
        let mut inp = Cursor::new(data);
        let req = Request::parse(&mut inp).unwrap().unwrap();
        assert_eq!(req.path, "/script.ws");
        assert_eq!(req.headers.find_first("Host"), Some(&b"foo.com"[..]));
    }

    #[test]
    fn request_replying() {
        const data: &'static [u8] = b"\
            GET /script.ws HTTP/1.1\r\n\
            Host: foo.com\r\n\
            Connection: upgrade\r\n\
            Upgrade: websocket\r\n\
            Sec-WebSocket-Version: 13\r\n\
            Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
            \r\n";
        let mut inp = Cursor::new(data);
        let req = Request::parse(&mut inp).unwrap().unwrap();
        let reply = req.reply().unwrap();
    }

}
