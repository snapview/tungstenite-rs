use std::io::{Read, Write, Cursor};

use base64;
use rand;
use bytes::Buf;
use httparse;
use httparse::Status;
use url::Url;

use input_buffer::{InputBuffer, MIN_READ};
use error::{Error, Result};
use protocol::{
    WebSocket, Role,
};
use super::{
    Headers,
    Httparse, FromHttparse,
    Handshake, HandshakeResult,
    convert_key,
    MAX_HEADERS,
};
use util::NonBlockingResult;

/// Client request.
pub struct Request {
    pub url: Url,
    // TODO extra headers
}

impl Request {
    /// The GET part of the request.
    fn get_path(&self) -> String {
        if let Some(query) = self.url.query() {
            format!("{path}?{query}", path = self.url.path(), query = query)
        } else {
            self.url.path().into()
        }
    }
    /// The Host: part of the request.
    fn get_host(&self) -> String {
        let host = self.url.host_str().expect("Bug: URL without host");
        if let Some(port) = self.url.port() {
            format!("{host}:{port}", host = host, port = port)
        } else {
            host.into()
        }
    }
}

/// Client handshake.
pub struct ClientHandshake<Stream> {
    stream: Stream,
    state: HandshakeState,
    verify_data: VerifyData,
}

impl<Stream: Read + Write> ClientHandshake<Stream> {
    /// Initiate a WebSocket handshake over the given stream.
    pub fn new(stream: Stream, request: Request) -> Self {
        let key = generate_key();

        let mut req = Vec::new();
        write!(req, "\
            GET {path} HTTP/1.1\r\n\
            Host: {host}\r\n\
            Connection: upgrade\r\n\
            Upgrade: websocket\r\n\
            Sec-WebSocket-Version: 13\r\n\
            Sec-WebSocket-Key: {key}\r\n\
            \r\n", host = request.get_host(), path = request.get_path(), key = key)
            .unwrap();

        let accept_key = convert_key(key.as_ref()).unwrap();

        ClientHandshake {
            stream: stream,
            state: HandshakeState::SendingRequest(Cursor::new(req)),
            verify_data: VerifyData {
                accept_key: accept_key,
            },
        }
    }
}

impl<Stream: Read + Write> Handshake for ClientHandshake<Stream> {
    type Stream = WebSocket<Stream>;
    fn handshake(mut self) -> Result<HandshakeResult<Self>> {
        debug!("Performing client handshake...");
        match self.state {
            HandshakeState::SendingRequest(mut req) => {
                let size = self.stream.write(Buf::bytes(&req)).no_block()?.unwrap_or(0);
                Buf::advance(&mut req, size);
                let state = if req.has_remaining() {
                    HandshakeState::SendingRequest(req)
                } else {
                    HandshakeState::ReceivingResponse(InputBuffer::with_capacity(MIN_READ))
                };
                Ok(HandshakeResult::Incomplete(ClientHandshake {
                    state: state,
                    ..self
                }))
            }
            HandshakeState::ReceivingResponse(mut resp_buf) => {
                resp_buf.reserve(MIN_READ, usize::max_value())
                    .map_err(|_| Error::Capacity("Header too long".into()))?;
                resp_buf.read_from(&mut self.stream).no_block()?;
                if let Some(resp) = Response::parse(&mut resp_buf)? {
                    self.verify_data.verify_response(&resp)?;
                    let ws = WebSocket::from_partially_read(self.stream,
                        resp_buf.into_vec(), Role::Client);
                    debug!("Client handshake done.");
                    Ok(HandshakeResult::Done(ws))
                } else {
                    Ok(HandshakeResult::Incomplete(ClientHandshake {
                        state: HandshakeState::ReceivingResponse(resp_buf),
                        ..self
                    }))
                }
            }
        }
    }
}

/// Information for handshake verification.
struct VerifyData {
    /// Accepted server key.
    accept_key: String,
}

impl VerifyData {
    pub fn verify_response(&self, response: &Response) -> Result<()> {
        // 1. If the status code received from the server is not 101, the
        // client handles the response per HTTP [RFC2616] procedures. (RFC 6455)
        if response.code != 101 {
            return Err(Error::Http(response.code));
        }
        // 2. If the response lacks an |Upgrade| header field or the |Upgrade|
        // header field contains a value that is not an ASCII case-
        // insensitive match for the value "websocket", the client MUST
        // _Fail the WebSocket Connection_. (RFC 6455)
        if !response.headers.header_is_ignore_case("Upgrade", "websocket") {
            return Err(Error::Protocol("No \"Upgrade: websocket\" in server reply".into()));
        }
        // 3.  If the response lacks a |Connection| header field or the
        // |Connection| header field doesn't contain a token that is an
        // ASCII case-insensitive match for the value "Upgrade", the client
        // MUST _Fail the WebSocket Connection_. (RFC 6455)
        if !response.headers.header_is_ignore_case("Connection", "Upgrade") {
            return Err(Error::Protocol("No \"Connection: upgrade\" in server reply".into()));
        }
        // 4.  If the response lacks a |Sec-WebSocket-Accept| header field or
        // the |Sec-WebSocket-Accept| contains a value other than the
        // base64-encoded SHA-1 of ... the client MUST _Fail the WebSocket
        // Connection_. (RFC 6455)
        if !response.headers.header_is("Sec-WebSocket-Accept", &self.accept_key) {
            return Err(Error::Protocol("Key mismatch in Sec-WebSocket-Accept".into()));
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
        // TODO

        Ok(())
    }
}

/// Internal state of the client handshake.
enum HandshakeState {
    SendingRequest(Cursor<Vec<u8>>),
    ReceivingResponse(InputBuffer),
}

/// Server response.
pub struct Response {
    code: u16,
    headers: Headers,
}

impl Response {
    /// Parse the response from a stream.
    pub fn parse<B: Buf>(input: &mut B) -> Result<Option<Self>> {
        Response::parse_http(input)
    }
}

impl Httparse for Response {
    fn httparse(buf: &[u8]) -> Result<Option<(usize, Self)>> {
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
            return Err(Error::Protocol("HTTP version should be 1.1 or higher".into()));
        }
        Ok(Response {
            code: raw.code.expect("Bug: no HTTP response code"),
            headers: Headers::from_httparse(raw.headers)?,
        })
    }
}

/// Generate a random key for the `Sec-WebSocket-Key` header.
fn generate_key() -> String {
    // a base64-encoded (see Section 4 of [RFC4648]) value that,
    // when decoded, is 16 bytes in length (RFC 6455)
    let r: [u8; 16] = rand::random();
    base64::encode(&r)
}

#[cfg(test)]
mod tests {

    use super::{Response, generate_key};

    use std::io::Cursor;

    #[test]
    fn random_keys() {
        let k1 = generate_key();
        println!("Generated random key 1: {}", k1);
        let k2 = generate_key();
        println!("Generated random key 2: {}", k2);
        assert_ne!(k1, k2);
        assert_eq!(k1.len(), k2.len());
        assert_eq!(k1.len(), 24);
        assert_eq!(k2.len(), 24);
        assert!(k1.ends_with("=="));
        assert!(k2.ends_with("=="));
        assert!(k1[..22].find("=").is_none());
        assert!(k2[..22].find("=").is_none());
    }

    #[test]
    fn response_parsing() {
        const data: &'static [u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n";
        let mut inp = Cursor::new(data);
        let req = Response::parse(&mut inp).unwrap().unwrap();
        assert_eq!(req.code, 200);
        assert_eq!(req.headers.find_first("Content-Type"), Some(&b"text/html"[..]));
    }

}
