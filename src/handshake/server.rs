//! Server handshake machine.

use std::fmt::Write as FmtWrite;
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::mem::replace;

use httparse;
use httparse::Status;

use error::{Error, Result};
use protocol::{WebSocket, Role};
use super::headers::{Headers, FromHttparse, MAX_HEADERS};
use super::machine::{HandshakeMachine, StageResult, TryParse};
use super::{MidHandshake, HandshakeRole, ProcessingResult, convert_key};

/// Request from the client.
pub struct Request {
    /// Path part of the URL.
    pub path: String,
    /// HTTP headers.
    pub headers: Headers,
}

impl Request {
    /// Reply to the response.
    pub fn reply(&self, extra_headers: Option<Vec<(String, String)>>) -> Result<Vec<u8>> {
        let key = self.headers.find_first("Sec-WebSocket-Key")
            .ok_or_else(|| Error::Protocol("Missing Sec-WebSocket-Key".into()))?;
        let mut reply = format!(
            "\
            HTTP/1.1 101 Switching Protocols\r\n\
            Connection: Upgrade\r\n\
            Upgrade: websocket\r\n\
            Sec-WebSocket-Accept: {}\r\n",
            convert_key(key)?
        );
        if let Some(eh) = extra_headers {
            for (k, v) in eh {
                write!(reply, "{}: {}\r\n", k, v).unwrap();
            }
        }
        write!(reply, "\r\n").unwrap();
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

/// The callback type, the callback is called when the server receives an incoming WebSocket
/// handshake request from the client, specifying a callback allows you to analyze incoming headers
/// and add additional headers to the response that server sends to the client and/or reject the
/// connection based on the incoming headers. Due to usability problems which are caused by a
/// static dispatch when using callbacks in such places, the callback is boxed.
///
/// The type uses `FnMut` instead of `FnOnce` as it is impossible to box `FnOnce` in the current
/// Rust version, `FnBox` is still unstable, this code has to be updated for `FnBox` when it gets
/// stable.
pub type Callback = Box<FnMut(&Request) -> Result<Option<Vec<(String, String)>>>>;

/// Server handshake role.
#[allow(missing_copy_implementations)]
pub struct ServerHandshake<S> {
    /// Callback which is called whenever the server read the request from the client and is ready
    /// to reply to it. The callback returns an optional headers which will be added to the reply
    /// which the server sends to the user.
    callback: Option<Callback>,
    /// Internal stream type.
    _marker: PhantomData<S>,
}

impl<S: Read + Write> ServerHandshake<S> {
    /// Start server handshake. `callback` specifies a custom callback which the user can pass to
    /// the handshake, this callback will be called when the a websocket client connnects to the
    /// server, you can specify the callback if you want to add additional header to the client
    /// upon join based on the incoming headers.
    pub fn start(stream: S, callback: Option<Callback>) -> MidHandshake<Self> {
        trace!("Server handshake initiated.");
        MidHandshake {
            machine: HandshakeMachine::start_read(stream),
            role: ServerHandshake { callback, _marker: PhantomData },
        }
    }
}

impl<S: Read + Write> HandshakeRole for ServerHandshake<S> {
    type IncomingData = Request;
    type InternalStream = S;
    type FinalResult = WebSocket<S>;

    fn stage_finished(&mut self, finish: StageResult<Self::IncomingData, Self::InternalStream>)
        -> Result<ProcessingResult<Self::InternalStream, Self::FinalResult>>
    {
        Ok(match finish {
            StageResult::DoneReading { stream, result, tail } => {
                if !tail.is_empty() {
                    return Err(Error::Protocol("Junk after client request".into()))
                }
                let extra_headers = {
                    if let Some(mut callback) = replace(&mut self.callback, None) {
                        callback(&result)?
                    } else {
                        None
                    }
                };
                let response = result.reply(extra_headers)?;
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
    use super::super::client::Response;

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
        let _ = req.reply(None).unwrap();

        let extra_headers = Some(vec![(String::from("MyCustomHeader"),
                                       String::from("MyCustomValue")),
                                       (String::from("MyVersion"),
                                        String::from("LOL"))]);
        let reply = req.reply(extra_headers).unwrap();
        let (_, req) = Response::try_parse(&reply).unwrap().unwrap();
        assert_eq!(req.headers.find_first("MyCustomHeader"), Some(b"MyCustomValue".as_ref()));
        assert_eq!(req.headers.find_first("MyVersion"), Some(b"LOL".as_ref()));
    }
}
