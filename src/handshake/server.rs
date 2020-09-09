//! Server handshake machine.

use std::io::{self, Read, Write};
use std::marker::PhantomData;
use std::result::Result as StdResult;

use http::{HeaderMap, Request as HttpRequest, Response as HttpResponse, StatusCode};
use httparse::Status;
use log::*;

use super::headers::{FromHttparse, MAX_HEADERS};
use super::machine::{HandshakeMachine, StageResult, TryParse};
use super::{convert_key, HandshakeRole, MidHandshake, ProcessingResult};
use crate::error::{Error, Result};
use crate::ext::WebSocketExtension;
use crate::protocol::{Role, WebSocket, WebSocketConfig};

/// Server request type.
pub type Request = HttpRequest<()>;

/// Server response type.
pub type Response = HttpResponse<()>;

/// Server error response type.
pub type ErrorResponse = HttpResponse<Option<String>>;

/// Create a response for the request.
pub fn create_response(request: &Request) -> Result<Response> {
    if request.method() != http::Method::GET {
        return Err(Error::Protocol("Method is not GET".into()));
    }

    if request.version() < http::Version::HTTP_11 {
        return Err(Error::Protocol(
            "HTTP version should be 1.1 or higher".into(),
        ));
    }

    if !request
        .headers()
        .get("Connection")
        .and_then(|h| h.to_str().ok())
        .map(|h| {
            h.split(|c| c == ' ' || c == ',')
                .any(|p| p.eq_ignore_ascii_case("Upgrade"))
        })
        .unwrap_or(false)
    {
        return Err(Error::Protocol(
            "No \"Connection: upgrade\" in client request".into(),
        ));
    }

    if !request
        .headers()
        .get("Upgrade")
        .and_then(|h| h.to_str().ok())
        .map(|h| h.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
    {
        return Err(Error::Protocol(
            "No \"Upgrade: websocket\" in client request".into(),
        ));
    }

    if !request
        .headers()
        .get("Sec-WebSocket-Version")
        .map(|h| h == "13")
        .unwrap_or(false)
    {
        return Err(Error::Protocol(
            "No \"Sec-WebSocket-Version: 13\" in client request".into(),
        ));
    }

    let key = request
        .headers()
        .get("Sec-WebSocket-Key")
        .ok_or_else(|| Error::Protocol("Missing Sec-WebSocket-Key".into()))?;

    let builder = Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .version(request.version())
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Accept", convert_key(key.as_bytes())?);

    Ok(builder.body(())?)
}

// Assumes that this is a valid response
fn write_response<T>(w: &mut dyn io::Write, response: &HttpResponse<T>) -> Result<()> {
    writeln!(
        w,
        "{version:?} {status} {reason}\r",
        version = response.version(),
        status = response.status(),
        reason = response.status().canonical_reason().unwrap_or(""),
    )?;

    for (k, v) in response.headers() {
        writeln!(w, "{}: {}\r", k, v.to_str()?).unwrap();
    }

    writeln!(w, "\r")?;

    Ok(())
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
        request: &Request,
        response: Response,
    ) -> StdResult<Response, ErrorResponse>;
}

impl<F> Callback for F
where
    F: FnOnce(&Request, Response) -> StdResult<Response, ErrorResponse>,
{
    fn on_request(
        self,
        request: &Request,
        response: Response,
    ) -> StdResult<Response, ErrorResponse> {
        self(request, response)
    }
}

/// Stub for callback that does nothing.
#[derive(Clone, Copy, Debug)]
pub struct NoCallback;

impl Callback for NoCallback {
    fn on_request(
        self,
        _request: &Request,
        response: Response,
    ) -> StdResult<Response, ErrorResponse> {
        Ok(response)
    }
}

/// Server handshake role.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct ServerHandshake<S, C, E>
where
    E: WebSocketExtension,
{
    /// Callback which is called whenever the server read the request from the client and is ready
    /// to reply to it. The callback returns an optional headers which will be added to the reply
    /// which the server sends to the user.
    callback: Option<C>,
    /// WebSocket configuration.
    config: Option<WebSocketConfig<E>>,
    /// Error code/flag. If set, an error will be returned after sending response to the client.
    error_code: Option<u16>,
    /// Internal stream type.
    _marker: PhantomData<S>,
}

impl<S: Read + Write, C: Callback, E> ServerHandshake<S, C, E>
where
    E: WebSocketExtension,
{
    /// Start server handshake. `callback` specifies a custom callback which the user can pass to
    /// the handshake, this callback will be called when the a websocket client connnects to the
    /// server, you can specify the callback if you want to add additional header to the client
    /// upon join based on the incoming headers.
    pub fn start(stream: S, callback: C, config: Option<WebSocketConfig<E>>) -> MidHandshake<Self> {
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

impl<S: Read + Write, C: Callback, E> HandshakeRole for ServerHandshake<S, C, E>
where
    E: WebSocketExtension,
{
    type IncomingData = Request;
    type InternalStream = S;
    type FinalResult = WebSocket<S, E>;

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

                let response = create_response(&result)?;
                let callback_result = if let Some(callback) = self.callback.take() {
                    callback.on_request(&result, response)
                } else {
                    Ok(response)
                };

                match callback_result {
                    Ok(response) => {
                        let mut output = vec![];
                        write_response(&mut output, &response)?;
                        ProcessingResult::Continue(HandshakeMachine::start_write(stream, output))
                    }

                    Err(resp) => {
                        if resp.status().is_success() {
                            return Err(Error::Protocol(
                                "Custom response must not be successful".into(),
                            ));
                        }

                        self.error_code = Some(resp.status().as_u16());

                        let mut output = vec![];
                        write_response(&mut output, &resp)?;
                        if let Some(body) = resp.body() {
                            output.extend_from_slice(body.as_bytes());
                        }
                        ProcessingResult::Continue(HandshakeMachine::start_write(stream, output))
                    }
                }
            }

            StageResult::DoneWriting(stream) => {
                if let Some(err) = self.error_code.take() {
                    debug!("Server handshake failed.");
                    return Err(Error::Http(StatusCode::from_u16(err)?));
                } else {
                    debug!("Server handshake done.");
                    let websocket =
                        WebSocket::from_raw_socket(stream, Role::Server, self.config.clone());
                    ProcessingResult::Done(websocket)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::machine::TryParse;
    use super::create_response;
    use super::Request;

    #[test]
    fn request_parsing() {
        const DATA: &[u8] = b"GET /script.ws HTTP/1.1\r\nHost: foo.com\r\n\r\n";
        let (_, req) = Request::try_parse(DATA).unwrap().unwrap();
        assert_eq!(req.uri().path(), "/script.ws");
        assert_eq!(req.headers().get("Host").unwrap(), &b"foo.com"[..]);
    }

    #[test]
    fn request_replying() {
        const DATA: &[u8] = b"\
            GET /script.ws HTTP/1.1\r\n\
            Host: foo.com\r\n\
            Connection: upgrade\r\n\
            Upgrade: websocket\r\n\
            Sec-WebSocket-Version: 13\r\n\
            Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
            \r\n";
        let (_, req) = Request::try_parse(DATA).unwrap().unwrap();
        let response = create_response(&req).unwrap();

        assert_eq!(
            response.headers().get("Sec-WebSocket-Accept").unwrap(),
            b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=".as_ref()
        );
    }
}
