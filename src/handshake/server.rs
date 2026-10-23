//! Server handshake machine.

use std::{
    io::{self, Read, Write},
    marker::PhantomData,
    result::Result as StdResult,
};

use http::{
    header::HeaderValue, response::Builder, HeaderMap, Request as HttpRequest,
    Response as HttpResponse, StatusCode,
};
use httparse::Status;
use log::*;

use super::{
    derive_accept_key,
    headers::{FromHttparse, MAX_HEADERS},
    machine::{HandshakeMachine, StageResult, TryParse},
    HandshakeRole, MidHandshake, ProcessingResult,
};
use crate::{
    error::{Error, ProtocolError, Result},
    handshake::version_as_str,
    protocol::{Role, WebSocket, WebSocketConfig},
};

/// Server request type.
pub type Request = HttpRequest<()>;

/// Server response type.
pub type Response = HttpResponse<()>;

/// Server error response type.
pub type ErrorResponse = HttpResponse<Option<String>>;

fn create_parts<T>(request: &HttpRequest<T>) -> Result<Builder> {
    if request.method() != http::Method::GET {
        return Err(Error::Protocol(ProtocolError::WrongHttpMethod));
    }

    if request.version() < http::Version::HTTP_11 {
        return Err(Error::Protocol(ProtocolError::WrongHttpVersion));
    }

    if !request
        .headers()
        .get("Connection")
        .and_then(|h| h.to_str().ok())
        .map(|h| h.split([' ', ',']).any(|p| p.eq_ignore_ascii_case("Upgrade")))
        .unwrap_or(false)
    {
        return Err(Error::Protocol(ProtocolError::MissingConnectionUpgradeHeader));
    }

    if !request
        .headers()
        .get("Upgrade")
        .and_then(|h| h.to_str().ok())
        .map(|h| h.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
    {
        return Err(Error::Protocol(ProtocolError::MissingUpgradeWebSocketHeader));
    }

    if !request.headers().get("Sec-WebSocket-Version").map(|h| h == "13").unwrap_or(false) {
        return Err(Error::Protocol(ProtocolError::MissingSecWebSocketVersionHeader));
    }

    let key = request
        .headers()
        .get("Sec-WebSocket-Key")
        .ok_or(Error::Protocol(ProtocolError::MissingSecWebSocketKey))?;

    if !is_valid_sec_websocket_key(key) {
        return Err(Error::Protocol(ProtocolError::InvalidSecWebSocketKey));
    }

    let builder = Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .version(request.version())
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Accept", derive_accept_key(key.as_bytes()));

    Ok(builder)
}

fn is_valid_sec_websocket_key(key: &HeaderValue) -> bool {
    if key.len() != 24 {
        return false;
    }

    let Ok(decoded) = data_encoding::BASE64.decode(key.as_bytes()) else {
        return false;
    };

    decoded.len() == 16
}

/// Create a response for the request.
pub fn create_response(request: &Request) -> Result<Response> {
    Ok(create_parts(request)?.body(())?)
}

/// Create a response for the request with a custom body.
pub fn create_response_with_body<T1, T2>(
    request: &HttpRequest<T1>,
    generate_body: impl FnOnce() -> T2,
) -> Result<HttpResponse<T2>> {
    Ok(create_parts(request)?.body(generate_body())?)
}

/// Write `response` to the stream `w`.
pub fn write_response<T>(mut w: impl io::Write, response: &HttpResponse<T>) -> Result<()> {
    writeln!(
        w,
        "{version} {status}\r",
        version = version_as_str(response.version())?,
        status = response.status()
    )?;

    for (k, v) in response.headers() {
        writeln!(w, "{}: {}\r", k, v.to_str()?)?;
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
            return Err(Error::Protocol(ProtocolError::WrongHttpMethod));
        }

        if raw.version.expect("Bug: no HTTP version") < /*1.*/1 {
            return Err(Error::Protocol(ProtocolError::WrongHttpVersion));
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
pub struct ServerHandshake<S, C> {
    /// Callback which is called whenever the server read the request from the client and is ready
    /// to reply to it. The callback returns an optional headers which will be added to the reply
    /// which the server sends to the user.
    callback: Option<C>,
    /// WebSocket configuration.
    config: Option<WebSocketConfig>,
    /// Error code/flag. If set, an error will be returned after sending response to the client.
    error_response: Option<ErrorResponse>,
    /// Internal stream type.
    _marker: PhantomData<S>,
}

impl<S: Read + Write, C: Callback> ServerHandshake<S, C> {
    /// Start server handshake. `callback` specifies a custom callback which the user can pass to
    /// the handshake, this callback will be called when the a websocket client connects to the
    /// server, you can specify the callback if you want to add additional header to the client
    /// upon join based on the incoming headers.
    pub fn start(stream: S, callback: C, config: Option<WebSocketConfig>) -> MidHandshake<Self> {
        trace!("Server handshake initiated.");
        MidHandshake {
            machine: HandshakeMachine::start_read(stream),
            role: ServerHandshake {
                callback: Some(callback),
                config,
                error_response: None,
                _marker: PhantomData,
            },
        }
    }
}

impl<S: Read + Write, C: Callback> HandshakeRole for ServerHandshake<S, C> {
    type IncomingData = Request;
    type InternalStream = S;
    type FinalResult = WebSocket<S>;

    fn stage_finished(
        &mut self,
        finish: StageResult<Self::IncomingData, Self::InternalStream>,
    ) -> Result<ProcessingResult<Self::InternalStream, Self::FinalResult>> {
        Ok(match finish {
            StageResult::DoneReading { stream, result, tail } => {
                if !tail.is_empty() {
                    return Err(Error::Protocol(ProtocolError::JunkAfterRequest));
                }

                let response = create_response(&result)?;
                let callback_result = if let Some(callback) = self.callback.take() {
                    callback.on_request(&result, response)
                } else {
                    Ok(response)
                };

                match callback_result {
                    Ok(response) => {
                        #[cfg(feature = "deflate")]
                        let mut response = response;
                        #[cfg(feature = "deflate")]
                        if self.config.as_ref().is_some_and(|config| config.deflate.is_some()) {
                            // Built-in permessage-deflate is appended as the sole extension:
                            // the 101 must not also attest a selection this socket has no
                            // codec for. The callback keeps the field when deflate is off.
                            if response
                                .headers()
                                .contains_key(http::header::SEC_WEBSOCKET_EXTENSIONS)
                            {
                                return Err(Error::Protocol(ProtocolError::InvalidHeader(
                                    http::header::SEC_WEBSOCKET_EXTENSIONS.clone().into(),
                                )));
                            }
                            let offers = result
                                .headers()
                                .get_all(http::header::SEC_WEBSOCKET_EXTENSIONS)
                                .iter()
                                .cloned()
                                .collect::<Vec<_>>();
                            let (config, agreed) = self
                                .config
                                .take()
                                .expect("configured deflate has a websocket config")
                                .accept_deflate_offers(&offers);
                            self.config = Some(config);
                            if let Some(agreed) = agreed {
                                response
                                    .headers_mut()
                                    .append(http::header::SEC_WEBSOCKET_EXTENSIONS, agreed);
                            }
                        } else if crate::protocol::deflate::headers_select_deflate(
                            response.headers(),
                        )? {
                            // A callback cannot advertise permessage-deflate while it is
                            // disabled here: the socket would have no codec for the state
                            // the 101 claims.
                            return Err(Error::Protocol(ProtocolError::InvalidHeader(
                                http::header::SEC_WEBSOCKET_EXTENSIONS.clone().into(),
                            )));
                        }

                        let mut output = vec![];
                        write_response(&mut output, &response)?;
                        ProcessingResult::Continue(HandshakeMachine::start_write(stream, output))
                    }

                    Err(resp) => {
                        if resp.status().is_success() {
                            return Err(Error::Protocol(ProtocolError::CustomResponseSuccessful));
                        }

                        self.error_response = Some(resp);
                        let resp = self.error_response.as_ref().unwrap();

                        let mut output = vec![];
                        write_response(&mut output, resp)?;

                        if let Some(body) = resp.body() {
                            output.extend_from_slice(body.as_bytes());
                        }

                        ProcessingResult::Continue(HandshakeMachine::start_write(stream, output))
                    }
                }
            }

            StageResult::DoneWriting(stream) => {
                if let Some(err) = self.error_response.take() {
                    debug!("Server handshake failed.");

                    let (parts, body) = err.into_parts();
                    let body = body.map(|b| b.as_bytes().to_vec());
                    return Err(Error::Http(http::Response::from_parts(parts, body).into()));
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
    use super::{super::machine::TryParse, create_response, Request};
    use crate::error::{Error, ProtocolError};

    #[cfg(feature = "deflate")]
    mod deflate {
        use super::*;

        /// A duplex over a fixed request, so `accept_hdr_with_config` runs without a
        /// socket.
        #[derive(Debug)]
        struct MockStream {
            read: std::io::Cursor<Vec<u8>>,
            written: Vec<u8>,
        }

        impl std::io::Read for MockStream {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                std::io::Read::read(&mut self.read, buf)
            }
        }

        impl std::io::Write for MockStream {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.written.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        /// A callback that adds an extension header the client never offered.
        struct InjectDeflate(&'static str);

        impl super::super::Callback for InjectDeflate {
            fn on_request(
                self,
                _request: &Request,
                mut response: super::super::Response,
            ) -> std::result::Result<super::super::Response, super::super::ErrorResponse>
            {
                response.headers_mut().insert(
                    http::header::SEC_WEBSOCKET_EXTENSIONS,
                    http::HeaderValue::from_static(self.0),
                );
                Ok(response)
            }
        }

        /// The callback runs before negotiation, so it cannot *remove* an agreed
        /// header -- that direction is impossible by construction. It can still
        /// **inject** one, and only the explicit check catches that.
        ///
        /// Without it the `101` advertises compression while `accept_deflate_offers`
        /// correctly installs none, because the request offered nothing: the wire and
        /// the codec disagree with nothing red.
        ///
        /// The unrelated name is refused for a second reason: automatic
        /// permessage-deflate is appended as the sole selection, so the `101` never
        /// attests an extension this socket has no codec for.
        #[test]
        fn a_callback_injecting_an_extension_header_fails_the_handshake() {
            let request = "GET /script.ws HTTP/1.1\r\n\
                 Host: foo.com\r\n\
                 Connection: upgrade\r\n\
                 Upgrade: websocket\r\n\
                 Sec-WebSocket-Version: 13\r\n\
                 Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                 \r\n";
            for injected in ["permessage-deflate", "PerMessage-Deflate", "x-example"] {
                let stream = MockStream {
                    read: std::io::Cursor::new(request.as_bytes().to_vec()),
                    written: Vec::new(),
                };
                let err = crate::accept_hdr_with_config(
                    stream,
                    InjectDeflate(injected),
                    Some(crate::protocol::WebSocketConfig::default().enable_deflate()),
                )
                .expect_err("an injected extension header must fail the handshake");

                assert!(
                    format!("{err:?}").to_lowercase().contains("sec-websocket-extensions"),
                    "{injected}: must name the header it rejected -- {err:?}"
                );
            }
        }

        /// Even when compression is disabled locally, a callback cannot advertise
        /// PMD: the returned socket has no codec and would reject the first RSV1
        /// frame sent under the apparently successful negotiation.
        #[test]
        fn a_callback_cannot_advertise_deflate_when_it_is_not_configured() {
            let request = "GET /script.ws HTTP/1.1\r\n\
                 Host: foo.com\r\n\
                 Connection: upgrade\r\n\
                 Upgrade: websocket\r\n\
                 Sec-WebSocket-Version: 13\r\n\
                 Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                 Sec-WebSocket-Extensions: permessage-deflate\r\n\
                 \r\n";
            let stream = MockStream {
                read: std::io::Cursor::new(request.as_bytes().to_vec()),
                written: Vec::new(),
            };

            let err =
                crate::accept_hdr_with_config(stream, InjectDeflate("permessage-deflate"), None)
                    .expect_err("a response cannot advertise a codec the socket did not install");

            assert!(matches!(
                err,
                crate::handshake::HandshakeError::Failure(Error::Protocol(
                    ProtocolError::InvalidHeader(_)
                ))
            ));
        }

        /// The control: the identical callback and config, injecting nothing, must
        /// complete. Otherwise the row above could pass because this path always
        /// fails.
        #[test]
        fn control_the_same_path_without_injection_completes() {
            let request = "GET /script.ws HTTP/1.1\r\n\
                 Host: foo.com\r\n\
                 Connection: upgrade\r\n\
                 Upgrade: websocket\r\n\
                 Sec-WebSocket-Version: 13\r\n\
                 Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                 \r\n";
            let stream = MockStream {
                read: std::io::Cursor::new(request.as_bytes().to_vec()),
                written: Vec::new(),
            };
            let socket = crate::accept_with_config(
                stream,
                Some(crate::protocol::WebSocketConfig::default().enable_deflate()),
            )
            .expect("no injection, so the handshake completes");
            let response = String::from_utf8_lossy(&socket.get_ref().written).to_ascii_lowercase();
            assert!(response.contains("101 switching protocols"), "{response}");
            assert!(
                !response.contains("sec-websocket-extensions"),
                "nothing was offered, so nothing is echoed: {response}"
            );
        }

        /// The pairing the soft cap exists for: this crate's own client offers
        /// `client_max_window_bits` with no value, so a reduced-cap server binds it and
        /// reserves 2^11 bytes to decompress with. Against a client that omits the parameter
        /// the cap cannot bind at all -- RFC 7692 §7.2.2 forces a 32 KiB decoder there, which
        /// `a_reduced_cap_prefers_a_binding_offer_over_an_earlier_fallback` still covers.
        #[test]
        fn a_reduced_cap_server_binds_this_crates_own_offer() {
            use crate::protocol::{deflate::Settings, Role, WebSocketConfig};

            let offers = [Settings::default().offer()];
            let (config, response) = WebSocketConfig::default()
                .enable_deflate()
                .deflate_max_window_bits(Role::Client, 11)
                .accept_deflate_offers(&offers);

            assert_eq!(response.unwrap(), "permessage-deflate; client_max_window_bits=11");
            let agreed = config.deflate.expect("compression stays enabled");
            assert_eq!(agreed.client_max_window_bits, 11);
        }

        /// Under a reduced cap the server prefers an alternative that lets the response bind
        /// the peer, and falls back to a full-window one only when the whole list holds none.
        /// RFC 7692 §7.2.2 calls the offered parameter a hint for building the response, and
        /// §7.1.3 lets the server pick any supported offer, so wire order yields inside that
        /// preference -- but never between two offers of the same class, nor at the default cap.
        #[test]
        fn a_reduced_cap_prefers_a_binding_offer_over_an_earlier_fallback() {
            use crate::protocol::{deflate::Settings, Role, WebSocketConfig};

            fn accept(cap: u8, offers: &[&str]) -> (Option<Settings>, Option<http::HeaderValue>) {
                let offers: Vec<http::HeaderValue> =
                    offers.iter().map(|offer| offer.parse().unwrap()).collect();
                let (config, response) = WebSocketConfig::default()
                    .enable_deflate()
                    .deflate_max_window_bits(Role::Client, cap)
                    .accept_deflate_offers(&offers);
                (config.deflate, response)
            }

            // Offer four is the first that can bind -- two full-window ahead, one narrower behind.
            let (agreed, response) = accept(
                11,
                &[
                    "permessage-deflate",
                    "permessage-deflate; server_max_window_bits=12",
                    "permessage-deflate; client_max_window_bits=11; future=3",
                    "permessage-deflate; client_no_context_takeover; client_max_window_bits=11",
                    "permessage-deflate; client_max_window_bits=10",
                ],
            );
            assert_eq!(
                response.unwrap(),
                "permessage-deflate; client_no_context_takeover; client_max_window_bits=11"
            );
            let agreed = agreed.unwrap();
            assert!(agreed.client_no_context_takeover);
            assert_eq!(agreed.client_max_window_bits, 11);

            let (agreed, response) = accept(
                11,
                &["permessage-deflate", "permessage-deflate; server_no_context_takeover"],
            );
            assert_eq!(response.unwrap(), "permessage-deflate");
            assert_eq!(agreed.unwrap().client_max_window_bits, 15);

            // At cap 15 nothing outranks anything, so the first wins though the second would bind.
            let (agreed, response) = accept(
                15,
                &["permessage-deflate", "permessage-deflate; client_max_window_bits=10"],
            );
            assert_eq!(response.unwrap(), "permessage-deflate");
            assert_eq!(agreed.unwrap().client_max_window_bits, 15);

            for (offer, expected) in [
                ("permessage-deflate; client_max_window_bits", 11),
                ("permessage-deflate; client_max_window_bits=12", 11),
                ("permessage-deflate; client_max_window_bits=10", 10),
            ] {
                let (agreed, response) = accept(11, &[offer]);
                assert_eq!(
                    response.unwrap(),
                    format!("permessage-deflate; client_max_window_bits={expected}")
                );
                assert_eq!(agreed.unwrap().client_max_window_bits, expected);
            }

            let split = [
                "x-example; value=\"a,b;c\"".parse().unwrap(),
                "permessage-deflate".parse().unwrap(),
            ];
            let (config, response) =
                WebSocketConfig::default().enable_deflate().accept_deflate_offers(&split);
            assert_eq!(response.unwrap(), "permessage-deflate");
            assert!(config.deflate.is_some());

            let mixed_raw =
                [http::HeaderValue::from_bytes(b"x-example; value=\x80, PerMessage-Deflate")
                    .unwrap()];
            let (config, response) =
                WebSocketConfig::default().enable_deflate().accept_deflate_offers(&mixed_raw);
            assert_eq!(response.unwrap(), "permessage-deflate");
            assert!(config.deflate.is_some());
        }
    }

    fn request_with_key(key: &str) -> Request {
        let data = format!(
            "\
            GET /script.ws HTTP/1.1\r\n\
            Host: foo.com\r\n\
            Connection: upgrade\r\n\
            Upgrade: websocket\r\n\
            Sec-WebSocket-Version: 13\r\n\
            Sec-WebSocket-Key: {key}\r\n\
            \r\n"
        );

        let (_, req) = Request::try_parse(data.as_bytes()).unwrap().unwrap();
        req
    }

    fn assert_invalid_sec_websocket_key(key: &str) {
        let req = request_with_key(key);
        let err = create_response(&req).unwrap_err();
        assert!(matches!(err, Error::Protocol(ProtocolError::InvalidSecWebSocketKey)));
    }

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

    #[test]
    fn test_invalid_websocket_key_empty() {
        assert_invalid_sec_websocket_key("");
    }

    #[test]
    fn test_invalid_websocket_key_too_long() {
        assert_invalid_sec_websocket_key("dGhlIHNhbXBsZSBub25jZQ==AAAAAAAAAA");
    }

    #[test]
    fn test_invalid_websocket_key_base64_symbol() {
        assert_invalid_sec_websocket_key("dGhlIHNhbXBsZSBub25jZQ!!");
    }

    #[test]
    fn test_invalid_websocket_key_decoded_length() {
        assert_invalid_sec_websocket_key("AAAAAAAAAAAAAAAAAAAAAAAA");
    }

    #[test]
    fn test_valid_websocket_key() {
        let req = request_with_key("dGhlIHNhbXBsZSBub25jZQ==");
        assert!(create_response(&req).is_ok());
    }
}
