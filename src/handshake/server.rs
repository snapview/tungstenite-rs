//! Server handshake machine.

use std::fmt::Write as FmtWrite;
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::result::Result as StdResult;

use http::{HeaderMap, Request, Response, StatusCode};
use httparse::Status;
use log::*;

use super::headers::{FromHttparse, MAX_HEADERS};
use super::machine::{HandshakeMachine, StageResult, TryParse};
use super::{convert_key, HandshakeRole, MidHandshake, ProcessingResult};
use crate::error::{Error, Result};
use crate::protocol::{Role, WebSocket, WebSocketConfig};

/// Reply to the response.
fn reply(request: &Request<()>, extra_headers: Option<HeaderMap>) -> Result<Vec<u8>> {
    let key = request
        .headers()
        .get("Sec-WebSocket-Key")
        .ok_or_else(|| Error::Protocol("Missing Sec-WebSocket-Key".into()))?;
    let mut reply = format!(
        "\
         HTTP/1.1 101 Switching Protocols\r\n\
         Connection: Upgrade\r\n\
         Upgrade: websocket\r\n\
         Sec-WebSocket-Accept: {}\r\n",
        convert_key(key.as_bytes())?
    );
    add_headers(&mut reply, extra_headers.as_ref())?;
    Ok(reply.into())
}

fn add_headers(reply: &mut impl FmtWrite, extra_headers: Option<&HeaderMap>) -> Result<()> {
    if let Some(eh) = extra_headers {
        for (k, v) in eh {
            writeln!(reply, "{}: {}\r", k, v.to_str()?).unwrap();
        }
    }
    writeln!(reply, "\r").unwrap();

    Ok(())
}

impl TryParse for Request<()> {
    fn try_parse(buf: &[u8]) -> Result<Option<(usize, Self)>> {
        let mut hbuffer = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut req = httparse::Request::new(&mut hbuffer);
        Ok(match req.parse(buf)? {
            Status::Partial => None,
            Status::Complete(size) => Some((size, Request::from_httparse(req)?)),
        })
    }
}

impl<'h, 'b: 'h> FromHttparse<httparse::Request<'h, 'b>> for Request<()> {
    fn from_httparse(raw: httparse::Request<'h, 'b>) -> Result<Self> {
        if raw.method.expect("Bug: no method in header") != "GET" {
            return Err(Error::Protocol("Method is not GET".into()));
        }

        if raw.version.expect("Bug: no HTTP version") < /*1.*/1 {
            return Err(Error::Protocol(
                "HTTP version should be 1.1 or higher".into(),
            ));
        }

        let headers = HeaderMap::from_httparse(raw.headers)?;

        let mut request = Request::new(());
        *request.method_mut() = http::Method::GET;
        *request.headers_mut() = headers;
        *request.uri_mut() = raw.path.expect("Bug: no path in header").parse()?;
        // TODO: httparse only supports HTTP 0.9/1.0/1.1 but not HTTP 2.0
        // so the only valid value we could get in the response would be 1.1.
        *request.version_mut() = http::Version::HTTP_11;

        Ok(request)
    }
}

/// The callback trait.
///
/// The callback is called when the server receives an incoming WebSocket
/// handshake request from the client. Specifying a callback allows you to analyze incoming headers
/// and add additional headers to the response that server sends to the client and/or reject the
/// connection based on the incoming headers.
pub trait Callback: Sized {
    /// Called whenever the server read the request from the client and is ready to reply to it.
    /// May return additional reply headers.
    /// Returning an error resulting in rejecting the incoming connection.
    fn on_request(
        self,
        request: &Request<()>,
    ) -> StdResult<Option<HeaderMap>, Response<Option<String>>>;
}

impl<F> Callback for F
where
    F: FnOnce(&Request<()>) -> StdResult<Option<HeaderMap>, Response<Option<String>>>,
{
    fn on_request(
        self,
        request: &Request<()>,
    ) -> StdResult<Option<HeaderMap>, Response<Option<String>>> {
        self(request)
    }
}

/// Stub for callback that does nothing.
#[derive(Clone, Copy, Debug)]
pub struct NoCallback;

impl Callback for NoCallback {
    fn on_request(
        self,
        _request: &Request<()>,
    ) -> StdResult<Option<HeaderMap>, Response<Option<String>>> {
        Ok(None)
    }
}

/// Server handshake role.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct ServerHandshake<S, C> {
    /// Callback which is called whenever the server read the request from the client and is ready
    /// to reply to it. The callback returns an optional headers which will be added to the reply
    /// which the server sends to the user.
    callback: Option<C>,
    /// WebSocket configuration.
    config: Option<WebSocketConfig>,
    /// Error code/flag. If set, an error will be returned after sending response to the client.
    error_code: Option<u16>,
    /// Internal stream type.
    _marker: PhantomData<S>,
}

impl<S: Read + Write, C: Callback> ServerHandshake<S, C> {
    /// Start server handshake. `callback` specifies a custom callback which the user can pass to
    /// the handshake, this callback will be called when the a websocket client connnects to the
    /// server, you can specify the callback if you want to add additional header to the client
    /// upon join based on the incoming headers.
    pub fn start(stream: S, callback: C, config: Option<WebSocketConfig>) -> MidHandshake<Self> {
        trace!("Server handshake initiated.");
        MidHandshake {
            machine: HandshakeMachine::start_read(stream),
            role: ServerHandshake {
                callback: Some(callback),
                config,
                error_code: None,
                _marker: PhantomData,
            },
        }
    }
}

impl<S: Read + Write, C: Callback> HandshakeRole for ServerHandshake<S, C> {
    type IncomingData = Request<()>;
    type InternalStream = S;
    type FinalResult = WebSocket<S>;

    fn stage_finished(
        &mut self,
        finish: StageResult<Self::IncomingData, Self::InternalStream>,
    ) -> Result<ProcessingResult<Self::InternalStream, Self::FinalResult>> {
        Ok(match finish {
            StageResult::DoneReading {
                stream,
                result,
                tail,
            } => {
                if !tail.is_empty() {
                    return Err(Error::Protocol("Junk after client request".into()));
                }

                let callback_result = if let Some(callback) = self.callback.take() {
                    callback.on_request(&result)
                } else {
                    Ok(None)
                };

                match callback_result {
                    Ok(extra_headers) => {
                        let response = reply(&result, extra_headers)?;
                        ProcessingResult::Continue(HandshakeMachine::start_write(stream, response))
                    }

                    Err(resp) => {
                        if resp.status().is_success() {
                            return Err(Error::Protocol(
                                "Custom response must not be successful".into(),
                            ));
                        }

                        self.error_code = Some(resp.status().as_u16());
                        let mut response = format!(
                            "{version:?} {status} {reason}\r\n",
                            version = resp.version(),
                            status = resp.status().as_u16(),
                            reason = resp.status().canonical_reason().unwrap_or("")
                        );
                        add_headers(&mut response, Some(resp.headers()))?;
                        if let Some(body) = resp.body() {
                            response += &body;
                        }
                        ProcessingResult::Continue(HandshakeMachine::start_write(stream, response))
                    }
                }
            }

            StageResult::DoneWriting(stream) => {
                if let Some(err) = self.error_code.take() {
                    debug!("Server handshake failed.");
                    return Err(Error::Http(StatusCode::from_u16(err)?));
                } else {
                    debug!("Server handshake done.");
                    let websocket = WebSocket::from_raw_socket(stream, Role::Server, self.config);
                    ProcessingResult::Done(websocket)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::machine::TryParse;
    use super::reply;
    use super::{HeaderMap, Request};
    use http::header::HeaderName;
    use http::Response;

    #[test]
    fn request_parsing() {
        const DATA: &'static [u8] = b"GET /script.ws HTTP/1.1\r\nHost: foo.com\r\n\r\n";
        let (_, req) = Request::try_parse(DATA).unwrap().unwrap();
        assert_eq!(req.uri().path(), "/script.ws");
        assert_eq!(req.headers().get("Host").unwrap(), &b"foo.com"[..]);
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
        let _ = reply(&req, None).unwrap();

        let extra_headers = {
            let mut headers = HeaderMap::new();
            headers.insert(
                HeaderName::from_bytes(&b"MyCustomHeader"[..]).unwrap(),
                "MyCustomValue".parse().unwrap(),
            );
            headers.insert(
                HeaderName::from_bytes(&b"MyVersion"[..]).unwrap(),
                "LOL".parse().unwrap(),
            );

            headers
        };
        let reply = reply(&req, Some(extra_headers)).unwrap();
        let (_, req) = Response::try_parse(&reply).unwrap().unwrap();
        assert_eq!(
            req.headers().get("MyCustomHeader").unwrap(),
            b"MyCustomValue".as_ref()
        );
        assert_eq!(req.headers().get("MyVersion").unwrap(), b"LOL".as_ref());
    }
}
