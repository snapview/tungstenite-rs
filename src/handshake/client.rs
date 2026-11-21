//! Client handshake machine.

use std::{
    io::{Read, Write},
    marker::PhantomData,
};

use http::{
    header::HeaderName, HeaderMap, Request as HttpRequest, Response as HttpResponse, StatusCode,
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
    error::{Error, ProtocolError, Result, SubProtocolError, UrlError},
    handshake::version_as_str,
    protocol::{Role, WebSocket, WebSocketConfig},
};

/// Client request type.
pub type Request = HttpRequest<()>;

/// Client response type.
pub type Response = HttpResponse<Option<Vec<u8>>>;

/// Client handshake role.
#[derive(Debug)]
pub struct ClientHandshake<S> {
    verify_data: VerifyData,
    config: Option<WebSocketConfig>,
    _marker: PhantomData<S>,
}

impl<S: Read + Write> ClientHandshake<S> {
    /// Initiate a client handshake.
    pub fn start(
        stream: S,
        request: Request,
        config: Option<WebSocketConfig>,
    ) -> Result<MidHandshake<Self>> {
        #[cfg(feature = "deflate")]
        let mut request = request;
        if request.method() != http::Method::GET {
            return Err(Error::Protocol(ProtocolError::WrongHttpMethod));
        }

        if request.version() < http::Version::HTTP_11 {
            return Err(Error::Protocol(ProtocolError::WrongHttpVersion));
        }

        // Check the URI scheme: only ws or wss are supported
        let _ = crate::client::uri_mode(request.uri())?;

        let subprotocols = extract_subprotocols_from_request(&request)?;

        #[cfg(feature = "deflate")]
        if let Some(offer) = config.as_ref().and_then(|config| config.deflate.map(|d| d.offer())) {
            // Built-in permessage-deflate is negotiated as the sole extension. Tungstenite
            // has no extension registry, so a 101 answering a request that names another
            // one would attest a state this socket cannot honour. An embedder negotiating
            // its own extensions builds the offer itself instead.
            if request.headers().contains_key(http::header::SEC_WEBSOCKET_EXTENSIONS) {
                return Err(Error::Protocol(ProtocolError::InvalidHeader(
                    http::header::SEC_WEBSOCKET_EXTENSIONS.clone().into(),
                )));
            }
            request.headers_mut().append("Sec-WebSocket-Extensions", offer);
        }

        // Convert and verify the `http::Request` and turn it into the request as per RFC.
        // Also extract the key from it (it must be present in a correct request).
        let (request, key) = generate_request(request)?;

        let machine = HandshakeMachine::start_write(stream, request);

        let client = {
            let accept_key = derive_accept_key(key.as_ref());
            ClientHandshake {
                verify_data: VerifyData { accept_key, subprotocols },
                config,
                _marker: PhantomData,
            }
        };

        trace!("Client handshake initiated.");
        Ok(MidHandshake { role: client, machine })
    }
}

impl<S: Read + Write> HandshakeRole for ClientHandshake<S> {
    type IncomingData = Response;
    type InternalStream = S;
    type FinalResult = (WebSocket<S>, Response);
    fn stage_finished(
        &mut self,
        finish: StageResult<Self::IncomingData, Self::InternalStream>,
    ) -> Result<ProcessingResult<Self::InternalStream, Self::FinalResult>> {
        Ok(match finish {
            StageResult::DoneWriting(stream) => {
                ProcessingResult::Continue(HandshakeMachine::start_read(stream))
            }
            StageResult::DoneReading { stream, result, tail } => {
                let result = match self.verify_data.verify_response(result) {
                    Ok(r) => r,
                    Err(Error::Http(mut e)) => {
                        *e.body_mut() = Some(tail);
                        return Err(Error::Http(e));
                    }
                    Err(e) => return Err(e),
                };

                #[cfg(feature = "deflate")]
                {
                    let offered = self.config.as_ref().and_then(|config| config.deflate);
                    let agreed = self.verify_data.verify_deflate_response(&result, offered)?;
                    if let Some(config) = self.config.as_mut() {
                        config.deflate = agreed;
                    }
                }

                debug!("Client handshake done.");
                let websocket =
                    WebSocket::from_partially_read(stream, tail, Role::Client, self.config);
                ProcessingResult::Done((websocket, result))
            }
        })
    }
}

/// Verifies and generates a client WebSocket request from the original request and extracts a WebSocket key from it.
pub fn generate_request(mut request: Request) -> Result<(Vec<u8>, String)> {
    let mut req = Vec::new();
    write!(
        req,
        "GET {path} {version}\r\n",
        path = request.uri().path_and_query().ok_or(Error::Url(UrlError::NoPathOrQuery))?.as_str(),
        version = version_as_str(request.version())?,
    )
    .unwrap();

    // Headers that must be present in a correct request.
    const KEY_HEADERNAME: &str = "Sec-WebSocket-Key";
    const WEBSOCKET_HEADERS: [&str; 5] =
        ["Host", "Connection", "Upgrade", "Sec-WebSocket-Version", KEY_HEADERNAME];

    // We must extract a WebSocket key from a properly formed request or fail if it's not present.
    let key = request
        .headers()
        .get(KEY_HEADERNAME)
        .ok_or_else(|| {
            Error::Protocol(ProtocolError::InvalidHeader(
                HeaderName::from_bytes(KEY_HEADERNAME.as_bytes()).unwrap().into(),
            ))
        })?
        .to_str()?
        .to_owned();

    // We must check that all necessary headers for a valid request are present. Note that we have to
    // deal with the fact that some apps seem to have a case-sensitive check for headers which is not
    // correct and should not considered the correct behavior, but it seems like some apps ignore it.
    // `http` by default writes all headers in lower-case which is fine (and does not violate the RFC)
    // but some servers seem to be poorely written and ignore RFC.
    //
    // See similar problem in `hyper`: https://github.com/hyperium/hyper/issues/1492
    let headers = request.headers_mut();
    for &header in &WEBSOCKET_HEADERS {
        let value = headers.remove(header).ok_or_else(|| {
            Error::Protocol(ProtocolError::InvalidHeader(
                HeaderName::from_bytes(header.as_bytes()).unwrap().into(),
            ))
        })?;
        write!(
            req,
            "{header}: {value}\r\n",
            header = header,
            value = value.to_str().map_err(|err| {
                Error::Utf8(format!("{err} for header name '{header}' with value: {value:?}"))
            })?
        )
        .unwrap();
    }

    // Now we must ensure that the headers that we've written once are not anymore present in the map.
    // If they do, then the request is invalid (some headers are duplicated there for some reason).
    let websocket_headers_contains =
        |name| WEBSOCKET_HEADERS.iter().any(|h| h.eq_ignore_ascii_case(name));

    for (k, v) in headers {
        let mut name = k.as_str();

        // We have already written the necessary headers once (above) and removed them from the map.
        // If we encounter them again, then the request is considered invalid and error is returned.
        if websocket_headers_contains(name) {
            return Err(Error::Protocol(ProtocolError::InvalidHeader(k.clone().into())));
        }

        // Relates to the issue of some servers treating headers in a case-sensitive way, please see:
        // https://github.com/snapview/tungstenite-rs/pull/119 (original fix of the problem)
        if name == "sec-websocket-protocol" {
            name = "Sec-WebSocket-Protocol";
        }

        if name == "origin" {
            name = "Origin";
        }

        // Write header as raw bytes to support non-ASCII values.
        // HTTP headers are defined as octets (RFC 7230), not UTF-8 strings.
        req.extend_from_slice(name.as_bytes());
        req.extend_from_slice(b": ");
        req.extend_from_slice(v.as_bytes());
        req.extend_from_slice(b"\r\n");
    }

    req.extend_from_slice(b"\r\n");
    trace!("Request: {:?}", String::from_utf8_lossy(&req));
    Ok((req, key))
}

fn extract_subprotocols_from_request(request: &Request) -> Result<Option<Vec<String>>> {
    if let Some(subprotocols) = request.headers().get("Sec-WebSocket-Protocol") {
        Ok(Some(subprotocols.to_str()?.split(',').map(|s| s.trim().to_string()).collect()))
    } else {
        Ok(None)
    }
}

/// Information for handshake verification.
#[derive(Debug)]
struct VerifyData {
    /// Accepted server key.
    accept_key: String,

    /// Accepted subprotocols
    subprotocols: Option<Vec<String>>,
}

impl VerifyData {
    pub fn verify_response(&self, response: Response) -> Result<Response> {
        // 1. If the status code received from the server is not 101, the
        // client handles the response per HTTP [RFC2616] procedures. (RFC 6455)
        if response.status() != StatusCode::SWITCHING_PROTOCOLS {
            return Err(Error::Http(response.into()));
        }

        let headers = response.headers();

        // 2. If the response lacks an |Upgrade| header field or the |Upgrade|
        // header field contains a value that is not an ASCII case-
        // insensitive match for the value "websocket", the client MUST
        // _Fail the WebSocket Connection_. (RFC 6455)
        if !headers
            .get("Upgrade")
            .and_then(|h| h.to_str().ok())
            .map(|h| h.eq_ignore_ascii_case("websocket"))
            .unwrap_or(false)
        {
            return Err(Error::Protocol(ProtocolError::MissingUpgradeWebSocketHeader));
        }
        // 3.  If the response lacks a |Connection| header field or the
        // |Connection| header field doesn't contain a token that is an
        // ASCII case-insensitive match for the value "Upgrade", the client
        // MUST _Fail the WebSocket Connection_. (RFC 6455)
        if !headers
            .get("Connection")
            .and_then(|h| h.to_str().ok())
            .map(|h| h.eq_ignore_ascii_case("Upgrade"))
            .unwrap_or(false)
        {
            return Err(Error::Protocol(ProtocolError::MissingConnectionUpgradeHeader));
        }
        // 4.  If the response lacks a |Sec-WebSocket-Accept| header field or
        // the |Sec-WebSocket-Accept| contains a value other than the
        // base64-encoded SHA-1 of ... the client MUST _Fail the WebSocket
        // Connection_. (RFC 6455)
        if !headers.get("Sec-WebSocket-Accept").map(|h| h == &self.accept_key).unwrap_or(false) {
            return Err(Error::Protocol(ProtocolError::SecWebSocketAcceptKeyMismatch));
        }
        // 5.  If the response includes a |Sec-WebSocket-Extensions| header
        // field and this header field indicates the use of an extension
        // that was not present in the client's handshake (the server has
        // indicated an extension not requested by the client), the client
        // MUST _Fail the WebSocket Connection_. (RFC 6455)
        // TODO

        // 6.  If the response includes a |Sec-WebSocket-Protocol| header field
        // and this header field indicates the use of a subprotocol that was
        // not present in the client's handshake (the server has indicated a
        // subprotocol not requested by the client), the client MUST _Fail
        // the WebSocket Connection_. (RFC 6455)
        if headers.get("Sec-WebSocket-Protocol").is_none() && self.subprotocols.is_some() {
            return Err(Error::Protocol(ProtocolError::SecWebSocketSubProtocolError(
                SubProtocolError::NoSubProtocol,
            )));
        }

        if headers.get("Sec-WebSocket-Protocol").is_some() && self.subprotocols.is_none() {
            return Err(Error::Protocol(ProtocolError::SecWebSocketSubProtocolError(
                SubProtocolError::ServerSentSubProtocolNoneRequested,
            )));
        }

        if let Some(returned_subprotocol) = headers.get("Sec-WebSocket-Protocol") {
            if let Some(accepted_subprotocols) = &self.subprotocols {
                if !accepted_subprotocols.contains(&returned_subprotocol.to_str()?.to_string()) {
                    return Err(Error::Protocol(ProtocolError::SecWebSocketSubProtocolError(
                        SubProtocolError::InvalidSubProtocol,
                    )));
                }
            }
        }

        Ok(response)
    }

    #[cfg(feature = "deflate")]
    fn verify_deflate_response(
        &self,
        response: &Response,
        offered: Option<crate::protocol::deflate::Settings>,
    ) -> Result<Option<crate::protocol::deflate::Settings>> {
        match offered {
            Some(offered) => offered.accept_response(response.headers()),
            // Nothing was offered, so a selection is the peer's invention. Any other
            // extension here was negotiated by the embedder and is not ours to judge.
            None => {
                if crate::protocol::deflate::headers_select_deflate(response.headers())? {
                    return Err(Error::Protocol(ProtocolError::InvalidHeader(
                        http::header::SEC_WEBSOCKET_EXTENSIONS.clone().into(),
                    )));
                }
                Ok(None)
            }
        }
    }
}

impl TryParse for Response {
    fn try_parse(buf: &[u8]) -> Result<Option<(usize, Self)>> {
        let mut hbuffer = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut req = httparse::Response::new(&mut hbuffer);
        Ok(match req.parse(buf)? {
            Status::Partial => None,
            Status::Complete(size) => Some((size, Response::from_httparse(req)?)),
        })
    }
}

impl<'h, 'b: 'h> FromHttparse<httparse::Response<'h, 'b>> for Response {
    fn from_httparse(raw: httparse::Response<'h, 'b>) -> Result<Self> {
        if raw.version.expect("Bug: no HTTP version") < /*1.*/1 {
            return Err(Error::Protocol(ProtocolError::WrongHttpVersion));
        }

        let headers = HeaderMap::from_httparse(raw.headers)?;

        let mut response = Response::new(None);
        *response.status_mut() = StatusCode::from_u16(raw.code.expect("Bug: no HTTP status code"))?;
        *response.headers_mut() = headers;
        // TODO: httparse only supports HTTP 0.9/1.0/1.1 but not HTTP 2.0
        // so the only valid value we could get in the response would be 1.1.
        *response.version_mut() = http::Version::HTTP_11;

        Ok(response)
    }
}

/// Generate a random key for the `Sec-WebSocket-Key` header.
pub fn generate_key() -> String {
    // a base64-encoded (see Section 4 of [RFC4648]) value that,
    // when decoded, is 16 bytes in length (RFC 6455)
    let r: [u8; 16] = rand::random();
    data_encoding::BASE64.encode(&r)
}

#[cfg(test)]
mod tests {
    use super::{super::machine::TryParse, generate_key, generate_request, Response};
    use crate::client::IntoClientRequest;

    #[cfg(feature = "deflate")]
    mod deflate {
        use super::*;

        #[test]
        fn client_response_validation() {
            use super::super::VerifyData;
            use crate::{
                error::ProtocolError,
                protocol::{deflate::Settings, Role},
                Error,
            };

            let accept = crate::handshake::derive_accept_key(b"dGhlIHNhbXBsZSBub25jZQ==");
            let verify = VerifyData { accept_key: accept.clone(), subprotocols: None };
            let response_value = |header: Option<http::HeaderValue>, offered: Option<Settings>| {
                let mut response = http::Response::builder()
                    .status(101)
                    .header("Connection", "Upgrade")
                    .header("Upgrade", "websocket")
                    .header("Sec-WebSocket-Accept", accept.clone());
                if let Some(header) = header {
                    response = response.header("Sec-WebSocket-Extensions", header);
                }
                let response = verify.verify_response(response.body(None).unwrap())?;
                verify.verify_deflate_response(&response, offered)
            };
            let response = |header: Option<&str>, offered: Option<Settings>| {
                response_value(header.map(|value| value.parse().unwrap()), offered)
            };
            let invalid_for = |header, offered| {
                // The compact API deliberately collapses every invalid PMD response
                // to InvalidHeader. These rows pin acceptance and installed state,
                // not the private parser route that produced the error.
                matches!(
                    response(Some(header), offered),
                    Err(Error::Protocol(ProtocolError::InvalidHeader(_)))
                )
            };
            let invalid = |header| invalid_for(header, Some(Settings::default()));

            assert!(response(None, Some(Settings::default())).unwrap().is_none());
            assert!(response(Some(""), Some(Settings::default())).unwrap().is_none());
            assert!(invalid_for("permessage-deflate", None));
            assert!(invalid_for("permessage-deflate; x=\"unterminated", None));
            assert!(invalid_for("x-example; x=\"unterminated", None));
            assert!(response_value(
                Some(http::HeaderValue::from_bytes(b"x-example; x=\x80").unwrap()),
                None
            )
            .unwrap()
            .is_none());
            assert!(matches!(
                response_value(
                    Some(http::HeaderValue::from_bytes(b"permessage-deflate; x=\x80").unwrap()),
                    None
                ),
                Err(Error::Protocol(ProtocolError::InvalidHeader(_)))
            ));
            assert!(matches!(
                response_value(
                    Some(
                        http::HeaderValue::from_bytes(b"x-example; x=\x80, PerMessage-Deflate",)
                            .unwrap(),
                    ),
                    Some(Settings::default())
                ),
                Err(Error::Protocol(ProtocolError::InvalidHeader(_)))
            ));
            assert_eq!(
                response_value(
                    Some(http::HeaderValue::from_bytes(b"PerMessage-Deflate").unwrap()),
                    Some(Settings::default())
                )
                .unwrap(),
                Some(Settings::default())
            );
            assert!(invalid(";x"));
            assert!(invalid("permessage-deflate; client_max_window_bits"));
            assert!(invalid("permessage-deflate; client_max_window_bits=09"));
            assert!(invalid("permessage-deflate; client_max_window_bits=+9"));
            assert!(invalid(
                "permessage-deflate; client_max_window_bits=12; client_max_window_bits=12"
            ));
            let agreed = response(
                Some("permessage-deflate; server_max_window_bits=8"),
                Some(Settings::default()),
            )
            .unwrap()
            .unwrap();
            assert_eq!(agreed, Settings { server_max_window_bits: 8, ..Settings::default() });
            assert!(invalid_for(
                "permessage-deflate",
                Some(Settings::default().no_context_takeover(Role::Server, true))
            ));
            let agreed = response(
                Some("permessage-deflate"),
                Some(Settings::default().no_context_takeover(Role::Client, true)),
            )
            .unwrap()
            .unwrap();
            assert_eq!(agreed, Settings::default().no_context_takeover(Role::Client, true));

            let agreed = response(
                Some("permessage-deflate; client_no_context_takeover"),
                Some(Settings::default()),
            )
            .unwrap()
            .unwrap();
            assert_eq!(agreed, Settings::default().no_context_takeover(Role::Client, true));

            assert!(invalid("x-example; value=\"a,b;c\", permessage-deflate"));
            // Quoted and escaped parameter values still parse: the quoted pair `\0` stands
            // for the character `0`, so `"1\0"` is the two digits of a 10-bit window.
            let agreed = response(
                Some("permessage-deflate; server_no_context_takeover; server_max_window_bits=\"1\\0\""),
                Some(Settings::default()),
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                agreed,
                Settings::default()
                    .no_context_takeover(Role::Server, true)
                    .max_window_bits(Role::Server, 10)
            );

            let capped = Settings::default().max_window_bits(Role::Server, 12);
            assert!(invalid_for("permessage-deflate", Some(capped)));
            assert!(invalid_for("permessage-deflate; server_max_window_bits=13", Some(capped)));
            assert_eq!(
                response(Some("permessage-deflate; server_max_window_bits=12"), Some(capped))
                    .unwrap(),
                Some(capped)
            );

            // The offer always names `client_max_window_bits`, and never with the configured
            // cap: that cap bounds our own encoder, not what the peer may reserve to decode.
            let capped_client = Settings::default().max_window_bits(Role::Client, 12);
            for settings in [Settings::default(), capped_client] {
                assert_eq!(
                    settings.offer().to_str().unwrap(),
                    "permessage-deflate; client_max_window_bits"
                );
            }
            // Having invited a selection, honour it -- downwards only, so a server narrows
            // our encoder and can never widen it past what was configured.
            for (configured, selected, agreed) in [(15, 9, 9), (15, 15, 15), (9, 15, 9), (12, 9, 9)]
            {
                let offered = Settings::default().max_window_bits(Role::Client, configured);
                let answer = format!("permessage-deflate; client_max_window_bits={selected}");
                assert_eq!(
                    response(Some(&answer), Some(offered)).unwrap(),
                    Some(Settings::default().max_window_bits(Role::Client, agreed)),
                    "a cap of {configured} against a selected {selected} agrees on {agreed}"
                );
            }
            // RFC 7692 §7.1.2.2 lets a server select 8; flate2 builds no compressor that
            // narrow, and encoding at 9 would exceed what the server agreed to decode.
            for settings in [Settings::default(), capped_client] {
                assert!(invalid_for(
                    "permessage-deflate; client_max_window_bits=8",
                    Some(settings)
                ));
            }
            // The parser's range is the only rejection below 8, where flate2 would panic.
            for answer in [
                "permessage-deflate; client_max_window_bits=0",
                "permessage-deflate; client_max_window_bits=7",
                "permessage-deflate; client_max_window_bits=16",
            ] {
                assert!(invalid(answer), "{answer} selects a width outside 8..=15");
            }
            // A response omitting the parameter declares no constraint, so the cap stands.
            assert_eq!(
                response(Some("permessage-deflate"), Some(capped_client)).unwrap(),
                Some(capped_client),
                "the configured cap stays on our own encoder"
            );
            // The sole-extension rule holds across repeated header fields, not just within one.
            let mut headers = http::HeaderMap::new();
            headers.append("Sec-WebSocket-Extensions", "x-example".parse().unwrap());
            headers.append("Sec-WebSocket-Extensions", "permessage-deflate".parse().unwrap());
            assert!(matches!(
                Settings::default().accept_response(&headers),
                Err(Error::Protocol(ProtocolError::InvalidHeader(_)))
            ));
        }

        #[test]
        fn client_rejects_competing_deflate_offer() {
            use std::io::Cursor;

            use super::super::ClientHandshake;
            use crate::{error::ProtocolError, protocol::WebSocketConfig, Error};

            let mut request = "ws://localhost/path".into_client_request().unwrap();
            request.headers_mut().append(
                "Sec-WebSocket-Extensions",
                "x-example, permessage-deflate".parse().unwrap(),
            );
            let result = ClientHandshake::start(
                Cursor::new(Vec::new()),
                request,
                Some(WebSocketConfig::default().enable_deflate()),
            );
            assert!(matches!(result, Err(Error::Protocol(ProtocolError::InvalidHeader(_)))));

            let mut request = "ws://localhost/path".into_client_request().unwrap();
            request.headers_mut().append(
                "Sec-WebSocket-Extensions",
                http::HeaderValue::from_bytes(b"PerMessage-Deflate; x=\x80").unwrap(),
            );
            let result = ClientHandshake::start(
                Cursor::new(Vec::new()),
                request,
                Some(WebSocketConfig::default().enable_deflate()),
            );
            assert!(matches!(result, Err(Error::Protocol(ProtocolError::InvalidHeader(_)))));

            // An unrelated extension is refused for the same reason as a competing one.
            let mut request = "ws://localhost/path".into_client_request().unwrap();
            request.headers_mut().append("Sec-WebSocket-Extensions", "x-example".parse().unwrap());
            let result = ClientHandshake::start(
                Cursor::new(Vec::new()),
                request,
                Some(WebSocketConfig::default().enable_deflate()),
            );
            assert!(matches!(result, Err(Error::Protocol(ProtocolError::InvalidHeader(_)))));

            // Without the built-in offer the field is the embedder's, and start succeeds.
            let mut request = "ws://localhost/path".into_client_request().unwrap();
            request.headers_mut().append("Sec-WebSocket-Extensions", "x-example".parse().unwrap());
            ClientHandshake::start(Cursor::new(Vec::new()), request, None)
                .expect("an embedder keeps its own extension field when deflate is off");
        }

        #[test]
        fn client_rejects_unoffered_deflate_response() {
            use std::{io::Cursor, marker::PhantomData};

            use super::super::{
                super::machine::StageResult, ClientHandshake, HandshakeRole, VerifyData,
            };
            use crate::{error::ProtocolError, Error};

            let accept_key = crate::handshake::derive_accept_key(b"dGhlIHNhbXBsZSBub25jZQ==");
            let response = http::Response::builder()
                .status(101)
                .header("Connection", "Upgrade")
                .header("Upgrade", "websocket")
                .header("Sec-WebSocket-Accept", accept_key.clone())
                .header("Sec-WebSocket-Extensions", "permessage-deflate")
                .body(None)
                .unwrap();
            let mut handshake = ClientHandshake {
                verify_data: VerifyData { accept_key, subprotocols: None },
                config: None,
                _marker: PhantomData,
            };
            let result = handshake.stage_finished(StageResult::DoneReading {
                stream: Cursor::new(Vec::new()),
                result: response,
                tail: Vec::new(),
            });
            assert!(matches!(result, Err(Error::Protocol(ProtocolError::InvalidHeader(_)))));
        }
    }

    #[test]
    fn random_keys() {
        let k1 = generate_key();
        println!("Generated random key 1: {k1}");
        let k2 = generate_key();
        println!("Generated random key 2: {k2}");
        assert_ne!(k1, k2);
        assert_eq!(k1.len(), k2.len());
        assert_eq!(k1.len(), 24);
        assert_eq!(k2.len(), 24);
        assert!(k1.ends_with("=="));
        assert!(k2.ends_with("=="));
        assert!(k1[..22].find('=').is_none());
        assert!(k2[..22].find('=').is_none());
    }

    fn construct_expected(host: &str, key: &str) -> Vec<u8> {
        format!(
            "\
            GET /getCaseCount HTTP/1.1\r\n\
            Host: {host}\r\n\
            Connection: Upgrade\r\n\
            Upgrade: websocket\r\n\
            Sec-WebSocket-Version: 13\r\n\
            Sec-WebSocket-Key: {key}\r\n\
            \r\n"
        )
        .into_bytes()
    }

    #[test]
    fn request_formatting() {
        let request = "ws://localhost/getCaseCount".into_client_request().unwrap();
        let (request, key) = generate_request(request).unwrap();
        let correct = construct_expected("localhost", &key);
        assert_eq!(&request[..], &correct[..]);
    }

    #[test]
    fn request_formatting_with_host() {
        let request = "wss://localhost:9001/getCaseCount".into_client_request().unwrap();
        let (request, key) = generate_request(request).unwrap();
        let correct = construct_expected("localhost:9001", &key);
        assert_eq!(&request[..], &correct[..]);
    }

    #[test]
    fn request_formatting_with_at() {
        let request = "wss://user:pass@localhost:9001/getCaseCount".into_client_request().unwrap();
        let (request, key) = generate_request(request).unwrap();
        let correct = construct_expected("localhost:9001", &key);
        assert_eq!(&request[..], &correct[..]);
    }

    #[test]
    fn response_parsing() {
        const DATA: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n";
        let (_, resp) = Response::try_parse(DATA).unwrap().unwrap();
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(resp.headers().get("Content-Type").unwrap(), &b"text/html"[..],);
    }

    #[test]
    fn invalid_custom_request() {
        let request = http::Request::builder().method("GET").body(()).unwrap();
        assert!(generate_request(request).is_err());
    }

    #[test]
    fn request_with_non_ascii_header() {
        use http::header::HeaderValue;

        let mut request = "ws://localhost/path".into_client_request().unwrap();

        // Add a header with non-ASCII value (UTF-8 encoded "Montréal")
        let non_ascii_value = HeaderValue::from_bytes(b"Montr\xc3\xa9al").unwrap();
        request.headers_mut().insert("X-City", non_ascii_value);

        // This should succeed, not fail with UTF-8 error
        let result = generate_request(request);
        assert!(result.is_ok(), "generate_request should accept non-ASCII header values");

        let (req_bytes, _key) = result.unwrap();

        // Verify the complete header with non-ASCII value is preserved in the output
        let expected_header = b"x-city: Montr\xc3\xa9al\r\n";
        assert!(
            req_bytes.windows(expected_header.len()).any(|window| window == expected_header),
            "Request should contain the complete non-ASCII header value"
        );
    }

    #[test]
    fn request_with_latin1_header() {
        use http::header::HeaderValue;

        let mut request = "ws://localhost/path".into_client_request().unwrap();

        // Add a header with ISO-8859-1 (Latin-1) encoded value
        // This is NOT valid UTF-8 but is valid for HTTP headers
        let latin1_value = HeaderValue::from_bytes(b"caf\xe9").unwrap(); // "café" in Latin-1
        request.headers_mut().insert("X-Test", latin1_value);

        // This should succeed
        let result = generate_request(request);
        assert!(result.is_ok(), "generate_request should accept Latin-1 header values");

        let (req_bytes, _key) = result.unwrap();

        // Verify the raw bytes are preserved in the output
        let expected_header = b"x-test: caf\xe9\r\n";
        assert!(
            req_bytes.windows(expected_header.len()).any(|window| window == expected_header),
            "Request should preserve the raw Latin-1 bytes"
        );
    }
}
