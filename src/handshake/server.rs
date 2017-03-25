use httparse;
use httparse::Status;

//use input_buffer::{InputBuffer, MIN_READ};
use error::{Error, Result};
use protocol::{WebSocket, Role};
use super::headers::{Headers, FromHttparse, MAX_HEADERS};
use super::machine::{HandshakeMachine, StageResult, TryParse};
use super::{MidHandshake, HandshakeRole, ProcessingResult, convert_key};

/// Request from the client.
pub struct Request {
    pub path: String,
    pub headers: Headers,
}

impl Request {
    /// Reply to the response.
    pub fn reply(&self) -> Result<Vec<u8>> {
        let key = self.headers.find_first("Sec-WebSocket-Key")
            .ok_or_else(|| Error::Protocol("Missing Sec-WebSocket-Key".into()))?;
        let reply = format!("\
        HTTP/1.1 101 Switching Protocols\r\n\
        Connection: Upgrade\r\n\
        Upgrade: websocket\r\n\
        Sec-WebSocket-Accept: {}\r\n\
        \r\n", convert_key(key)?);
        Ok(reply.into())
    }
}

impl TryParse for Request {
    fn try_parse(buf: &[u8]) -> Result<Option<(usize, Self)>> {
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

/// Server handshake role.
#[allow(missing_copy_implementations)]
pub struct ServerHandshake;

impl ServerHandshake {
    /// Start server handshake.
    pub fn start<Stream>(stream: Stream) -> MidHandshake<Stream, Self> {
        trace!("Server handshake initiated.");
        MidHandshake {
            machine: HandshakeMachine::start_read(stream),
            role: ServerHandshake,
        }
    }
}

impl HandshakeRole for ServerHandshake {
    type IncomingData = Request;
    fn stage_finished<Stream>(&self, finish: StageResult<Self::IncomingData, Stream>)
        -> Result<ProcessingResult<Stream>>
    {
        Ok(match finish {
            StageResult::DoneReading { stream, result, tail } => {
                if ! tail.is_empty() {
                    return Err(Error::Protocol("Junk after client request".into()))
                }
                let response = result.reply()?;
                ProcessingResult::Continue(HandshakeMachine::start_write(stream, response))
            }
            StageResult::DoneWriting(stream) => {
                debug!("Server handshake done.");
                ProcessingResult::Done(WebSocket::from_raw_socket(stream, Role::Server))
            }
        })
    }
}

#[cfg(test)]
mod tests {

    use super::Request;
    use super::super::machine::TryParse;

    #[test]
    fn request_parsing() {
        const DATA: &'static [u8] = b"GET /script.ws HTTP/1.1\r\nHost: foo.com\r\n\r\n";
        let (_, req) = Request::try_parse(DATA).unwrap().unwrap();
        assert_eq!(req.path, "/script.ws");
        assert_eq!(req.headers.find_first("Host"), Some(&b"foo.com"[..]));
    }

    #[test]
    fn request_replying() {
        const DATA: &'static [u8] = b"\
            GET /script.ws HTTP/1.1\r\n\
            Host: foo.com\r\n\
            Connection: upgrade\r\n\
            Upgrade: websocket\r\n\
            Sec-WebSocket-Version: 13\r\n\
            Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
            \r\n";
        let (_, req) = Request::try_parse(DATA).unwrap().unwrap();
        let _ = req.reply().unwrap();
    }

}
