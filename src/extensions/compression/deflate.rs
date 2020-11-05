//! Permessage-deflate extension

use std::fmt::{Display, Formatter};

use crate::extensions::compression::uncompressed::UncompressedExt;
use crate::extensions::WebSocketExtension;
use crate::protocol::frame::coding::{Data, OpCode};
use crate::protocol::frame::{ExtensionHeaders, Frame};
use crate::protocol::message::{IncompleteMessage, IncompleteMessageType};
use crate::protocol::MAX_MESSAGE_SIZE;
use crate::Message;
use bytes::BufMut;
use flate2::{
    Compress, CompressError, Compression, Decompress, DecompressError, FlushCompress,
    FlushDecompress, Status,
};
use http::header::{InvalidHeaderValue, SEC_WEBSOCKET_EXTENSIONS};
use http::{HeaderValue, Request, Response};
use std::borrow::Cow;
use std::mem::replace;
use std::slice;

/// The WebSocket Extension Identifier as per the IANA registry.
const EXT_IDENT: &str = "permessage-deflate";

/// The minimum size of the LZ77 sliding window size.
const LZ77_MIN_WINDOW_SIZE: u8 = 8;

/// The maximum size of the LZ77 sliding window size. Absence of the `max_window_bits` parameter
/// indicates that the client can receive messages compressed using an LZ77 sliding window of up to
/// 32,768 bytes. RFC 7692 7.1.2.1.
const LZ77_MAX_WINDOW_SIZE: u8 = 15;

/// A permessage-deflate configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeflateConfig {
    /// The maximum size of a message. The default value is 64 MiB which should be reasonably big
    /// for all normal use-cases but small enough to prevent memory eating by a malicious user.
    max_message_size: Option<usize>,
    /// The client's LZ77 sliding window size. Negotiated during the HTTP upgrade. In client mode,
    /// this conforms to RFC 7692 7.1.2.1. In server mode, this conforms to RFC 7692 7.1.2.2. Must
    /// be in range 8..15 inclusive.
    server_max_window_bits: u8,
    /// The client's LZ77 sliding window size. Negotiated during the HTTP upgrade. In client mode,
    /// this conforms to RFC 7692 7.1.2.2. In server mode, this conforms to RFC 7692 7.1.2.2. Must
    /// be in range 8..15 inclusive.
    client_max_window_bits: u8,
    /// Request that the server resets the LZ77 sliding window between messages - RFC 7692 7.1.1.1.
    request_no_context_takeover: bool,
    /// Whether to accept `no_context_takeover`.
    accept_no_context_takeover: bool,
    // Whether the compressor should be reset after usage.
    compress_reset: bool,
    // Whether the decompressor should be reset after usage.
    decompress_reset: bool,
    /// The active compression level. The integer here is typically on a scale of 0-9 where 0 means
    /// "no compression" and 9 means "take as long as you'd like".
    compression_level: Compression,
}

impl DeflateConfig {
    /// Builds a new `DeflateConfig` using the `compression_level` and the defaults for all other
    /// members.
    pub fn with_compression_level(compression_level: Compression) -> DeflateConfig {
        DeflateConfig {
            compression_level,
            ..Default::default()
        }
    }

    /// Returns the maximum message size permitted.
    pub fn max_message_size(&self) -> Option<usize> {
        self.max_message_size
    }

    /// Returns the maximum LZ77 window size permitted for the server.
    pub fn server_max_window_bits(&self) -> u8 {
        self.server_max_window_bits
    }

    /// Returns the maximum LZ77 window size permitted for the client.
    pub fn client_max_window_bits(&self) -> u8 {
        self.client_max_window_bits
    }

    /// Returns whether `no_context_takeover` has been requested.
    pub fn request_no_context_takeover(&self) -> bool {
        self.request_no_context_takeover
    }

    /// Returns whether this WebSocket will accept `no_context_takeover`.
    pub fn accept_no_context_takeover(&self) -> bool {
        self.accept_no_context_takeover
    }

    /// Returns whether or not the inner compressor is set to reset after completing a message.
    pub fn compress_reset(&self) -> bool {
        self.compress_reset
    }

    /// Returns whether or not the inner decompressor is set to reset after completing a message.
    pub fn decompress_reset(&self) -> bool {
        self.decompress_reset
    }

    /// Returns the active compression level.
    pub fn compression_level(&self) -> Compression {
        self.compression_level
    }

    /// Sets the maximum message size permitted.
    pub fn set_max_message_size(&mut self, max_message_size: Option<usize>) {
        self.max_message_size = max_message_size;
    }

    /// Sets the LZ77 sliding window size.
    pub fn set_max_window_bits(&mut self, max_window_bits: u8) {
        assert!((LZ77_MIN_WINDOW_SIZE..=LZ77_MAX_WINDOW_SIZE).contains(&max_window_bits));
        self.client_max_window_bits = max_window_bits;
    }

    /// Sets the WebSocket to request `no_context_takeover` if `true`.
    pub fn set_request_no_context_takeover(&mut self, request_no_context_takeover: bool) {
        self.request_no_context_takeover = request_no_context_takeover;
    }

    /// Sets the WebSocket to accept `no_context_takeover` if `true`.
    pub fn set_accept_no_context_takeover(&mut self, accept_no_context_takeover: bool) {
        self.accept_no_context_takeover = accept_no_context_takeover;
    }

    #[cfg(test)]
    pub fn set_compress_reset(&mut self, compress_reset: bool) {
        self.compress_reset = compress_reset
    }

    #[cfg(test)]
    pub fn set_decompress_reset(&mut self, decompress_reset: bool) {
        self.decompress_reset = decompress_reset;
    }
}

impl Default for DeflateConfig {
    fn default() -> Self {
        DeflateConfig {
            max_message_size: Some(MAX_MESSAGE_SIZE),
            server_max_window_bits: LZ77_MAX_WINDOW_SIZE,
            client_max_window_bits: LZ77_MAX_WINDOW_SIZE,
            request_no_context_takeover: false,
            accept_no_context_takeover: true,
            compress_reset: false,
            decompress_reset: false,
            compression_level: Compression::best(),
        }
    }
}

/// A `DeflateConfig` builder.
#[derive(Debug, Copy, Clone)]
pub struct DeflateConfigBuilder {
    max_message_size: Option<usize>,
    server_max_window_bits: u8,
    client_max_window_bits: u8,
    request_no_context_takeover: bool,
    accept_no_context_takeover: bool,
    fragments_grow: bool,
    compression_level: Compression,
}

impl Default for DeflateConfigBuilder {
    fn default() -> Self {
        DeflateConfigBuilder {
            max_message_size: Some(MAX_MESSAGE_SIZE),
            server_max_window_bits: LZ77_MAX_WINDOW_SIZE,
            client_max_window_bits: LZ77_MAX_WINDOW_SIZE,
            request_no_context_takeover: false,
            accept_no_context_takeover: true,
            fragments_grow: true,
            compression_level: Compression::fast(),
        }
    }
}

impl DeflateConfigBuilder {
    /// Sets the maximum message size permitted.
    pub fn max_message_size(mut self, max_message_size: Option<usize>) -> DeflateConfigBuilder {
        self.max_message_size = max_message_size;
        self
    }

    /// Sets the server's LZ77 sliding window size. Panics if the provided size is not in `8..=15`.
    pub fn server_max_window_bits(mut self, max_window_bits: u8) -> DeflateConfigBuilder {
        assert!(
            (LZ77_MIN_WINDOW_SIZE..=LZ77_MAX_WINDOW_SIZE).contains(&max_window_bits),
            "max window bits must be in range 8..=15"
        );
        self.server_max_window_bits = max_window_bits;
        self
    }

    /// Sets the client's LZ77 sliding window size. Panics if the provided size is not in `8..=15`.
    pub fn client_max_window_bits(mut self, max_window_bits: u8) -> DeflateConfigBuilder {
        assert!(
            (LZ77_MIN_WINDOW_SIZE..=LZ77_MAX_WINDOW_SIZE).contains(&max_window_bits),
            "max window bits must be in range 8..=15"
        );
        self.client_max_window_bits = max_window_bits;
        self
    }

    /// Sets the WebSocket to request `no_context_takeover`.
    pub fn request_no_context_takeover(
        mut self,
        request_no_context_takeover: bool,
    ) -> DeflateConfigBuilder {
        self.request_no_context_takeover = request_no_context_takeover;
        self
    }

    /// Sets the WebSocket to accept `no_context_takeover`.
    pub fn accept_no_context_takeover(
        mut self,
        accept_no_context_takeover: bool,
    ) -> DeflateConfigBuilder {
        self.accept_no_context_takeover = accept_no_context_takeover;
        self
    }

    /// Consumes the builder and produces a `DeflateConfig.`
    pub fn build(self) -> DeflateConfig {
        DeflateConfig {
            max_message_size: self.max_message_size,
            server_max_window_bits: self.server_max_window_bits,
            client_max_window_bits: self.client_max_window_bits,
            request_no_context_takeover: self.request_no_context_takeover,
            accept_no_context_takeover: self.accept_no_context_takeover,
            compression_level: self.compression_level,
            ..Default::default()
        }
    }
}

/// A permessage-deflate encoding WebSocket extension.
#[derive(Debug)]
pub struct DeflateExt {
    /// The configuration for the extension.
    config: DeflateConfig,
    /// A stack of continuation frames awaiting `fin` and the total size of all of the fragments.
    fragment_buffer: FragmentBuffer,
    /// The deflate decompressor.
    inflator: Inflator,
    /// The deflate compressor.
    deflator: Deflator,
    /// If this deflate extension is not used, messages will be forwarded to this extension.
    uncompressed_extension: UncompressedExt,
}

impl DeflateExt {
    /// Creates a `DeflateExt` instance for a client using the provided configuration.
    pub fn client(config: DeflateConfig) -> DeflateExt {
        DeflateExt {
            config,
            fragment_buffer: FragmentBuffer::new(config.max_message_size),
            inflator: Inflator::new(config.server_max_window_bits),
            deflator: Deflator::new(config.compression_level, config.client_max_window_bits),
            uncompressed_extension: UncompressedExt::new(config.max_message_size()),
        }
    }

    /// Creates a `DeflateExt` instance for a server using the provided configuration.
    pub fn server(config: DeflateConfig) -> DeflateExt {
        DeflateExt {
            config,
            fragment_buffer: FragmentBuffer::new(config.max_message_size),
            inflator: Inflator::new(config.client_max_window_bits),
            deflator: Deflator::new(config.compression_level, config.server_max_window_bits),
            uncompressed_extension: UncompressedExt::new(config.max_message_size()),
        }
    }
}

fn parse_window_parameter(
    window_param: &str,
    max_window_bits: u8,
) -> Result<Option<u8>, DeflateExtensionError> {
    let window_param = window_param.replace("\"", "");
    match window_param.trim().parse() {
        Ok(window_bits) => {
            if window_bits >= LZ77_MIN_WINDOW_SIZE && window_bits <= LZ77_MAX_WINDOW_SIZE {
                if window_bits != max_window_bits {
                    Ok(Some(window_bits))
                } else {
                    Ok(None)
                }
            } else {
                Err(DeflateExtensionError::InvalidMaxWindowBits)
            }
        }
        Err(_) => Err(DeflateExtensionError::InvalidMaxWindowBits),
    }
}

/// A permessage-deflate extension error.
#[derive(Debug, Clone, PartialEq)]
pub enum DeflateExtensionError {
    /// An error produced when deflating a message.
    DeflateError(String),
    /// An error produced when inflating a message.
    InflateError(String),
    /// An error produced during the WebSocket negotiation.
    NegotiationError(String),
    /// Produced when fragment buffer grew beyond the maximum configured size.
    Capacity(Cow<'static, str>),
    /// An invalid LZ77 window size was provided.
    InvalidMaxWindowBits,
}

impl DeflateExtensionError {
    fn malformatted() -> DeflateExtensionError {
        DeflateExtensionError::NegotiationError("Malformatted header value".into())
    }
}

impl Display for DeflateExtensionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DeflateExtensionError::DeflateError(m) => {
                write!(f, "An error was produced during decompression: {}", m)
            }
            DeflateExtensionError::InflateError(m) => {
                write!(f, "An error was produced during compression: {}", m)
            }
            DeflateExtensionError::NegotiationError(m) => {
                write!(f, "An upgrade error was encountered: {}", m)
            }
            DeflateExtensionError::Capacity(ref msg) => write!(f, "Space limit exceeded: {}", msg),
            DeflateExtensionError::InvalidMaxWindowBits => {
                write!(f, "An invalid window bit size was provided")
            }
        }
    }
}

/// Verifies any required Sec-WebSocket-Extension headers required for the configured compression
/// level from the HTTP response. Returns `Ok(true)` if a configuration could be agreed, `Ok(false)`
/// if the HTTP header was well formatted but no configuration could be agreed, or an error if it
/// was malformatted.
pub fn on_response<T>(
    response: &Response<T>,
    config: &mut DeflateConfig,
) -> Result<bool, DeflateExtensionError> {
    let mut seen_extension_name = false;
    let mut seen_server_takeover = false;
    let mut seen_client_takeover = false;
    let mut seen_server_max_window_bits = false;
    let mut seen_client_max_window_bits = false;
    let mut enabled = false;

    let DeflateConfig {
        server_max_window_bits,
        client_max_window_bits,
        accept_no_context_takeover,
        compress_reset,
        decompress_reset,
        ..
    } = config;

    for header in response.headers().get_all(SEC_WEBSOCKET_EXTENSIONS).iter() {
        match header.to_str() {
            Ok(header) => {
                for param in header.split(';') {
                    match param.trim().to_lowercase().as_str() {
                        EXT_IDENT => {
                            if seen_extension_name {
                                return Err(DeflateExtensionError::NegotiationError(format!(
                                    "Duplicate extension parameter: {}",
                                    EXT_IDENT
                                )));
                            } else {
                                enabled = true;
                                seen_extension_name = true;
                            }
                        }
                        "server_no_context_takeover" => {
                            if seen_server_takeover {
                                return Err(DeflateExtensionError::NegotiationError(format!(
                                    "Duplicate extension parameter: server_no_context_takeover"
                                )));
                            } else {
                                seen_server_takeover = true;
                                *decompress_reset = true;
                            }
                        }
                        "client_no_context_takeover" => {
                            if seen_client_takeover {
                                return Err(DeflateExtensionError::NegotiationError(format!(
                                    "Duplicate extension parameter: client_no_context_takeover"
                                )));
                            } else {
                                seen_client_takeover = true;

                                if *accept_no_context_takeover {
                                    *compress_reset = true;
                                } else {
                                    return Err(DeflateExtensionError::NegotiationError(format!(
                                        "The client requires context takeover."
                                    )));
                                }
                            }
                        }
                        param if param.starts_with("server_max_window_bits") => {
                            if seen_server_max_window_bits {
                                return Err(DeflateExtensionError::NegotiationError(format!(
                                    "Duplicate extension parameter: server_max_window_bits"
                                )));
                            } else {
                                seen_server_max_window_bits = true;

                                let mut window_param = param.split("=").skip(1);
                                match window_param.next() {
                                    Some(window_param) => {
                                        if let Some(bits) = parse_window_parameter(
                                            window_param,
                                            *server_max_window_bits,
                                        )? {
                                            *server_max_window_bits = bits;
                                        }
                                    }
                                    None => {
                                        return Err(DeflateExtensionError::InvalidMaxWindowBits)
                                    }
                                }
                            }
                        }
                        param if param.starts_with("client_max_window_bits") => {
                            if seen_client_max_window_bits {
                                return Err(DeflateExtensionError::NegotiationError(format!(
                                    "Duplicate extension parameter: client_max_window_bits"
                                )));
                            } else {
                                seen_client_max_window_bits = true;

                                let mut window_param = param.split("=").skip(1);
                                if let Some(window_param) = window_param.next() {
                                    if let Some(bits) = parse_window_parameter(
                                        window_param,
                                        *client_max_window_bits,
                                    )? {
                                        *client_max_window_bits = bits;
                                    }
                                }
                            }
                        }
                        p => {
                            return Err(DeflateExtensionError::NegotiationError(format!(
                                "Unknown permessage-deflate parameter: {}",
                                p
                            )));
                        }
                    }
                }
            }
            Err(e) => {
                return Err(DeflateExtensionError::NegotiationError(format!(
                    "Failed to parse extension parameter: {}",
                    e
                )));
            }
        }
    }

    Ok(enabled)
}

/// Applies the required headers to negotiate this PCME.
pub fn on_make_request<T>(mut request: Request<T>, config: &DeflateConfig) -> Request<T> {
    let mut header_value = String::from(EXT_IDENT);

    let DeflateConfig {
        server_max_window_bits,
        client_max_window_bits,
        request_no_context_takeover,
        ..
    } = config;

    if *client_max_window_bits < LZ77_MAX_WINDOW_SIZE
        || *server_max_window_bits < LZ77_MAX_WINDOW_SIZE
    {
        header_value.push_str(&format!(
            "; client_max_window_bits={}; server_max_window_bits={}",
            client_max_window_bits, server_max_window_bits
        ))
    } else {
        header_value.push_str("; client_max_window_bits")
    }

    if *request_no_context_takeover {
        header_value.push_str("; server_no_context_takeover")
    }

    request.headers_mut().append(
        SEC_WEBSOCKET_EXTENSIONS,
        HeaderValue::from_str(&header_value).unwrap(),
    );

    request
}

fn validate_req_extensions(
    header: &str,
    config: &mut DeflateConfig,
) -> Result<Option<String>, DeflateExtensionError> {
    let mut response_str = String::with_capacity(header.len());
    let mut param_iter = header.split(';');

    match param_iter.next() {
        Some(name) if name.trim() == EXT_IDENT => {
            response_str.push_str(EXT_IDENT);
        }
        _ => {
            return Ok(None);
        }
    }

    let mut server_takeover = false;
    let mut client_takeover = false;
    let mut server_max_bits = false;
    let mut client_max_bits = false;

    while let Some(param) = param_iter.next() {
        match param.trim().to_lowercase().as_str() {
            "server_no_context_takeover" => {
                if server_takeover {
                    return Err(DeflateExtensionError::malformatted());
                } else {
                    server_takeover = true;
                    if config.accept_no_context_takeover() {
                        config.compress_reset = true;
                        response_str.push_str("; server_no_context_takeover");
                    }
                }
            }
            "client_no_context_takeover" => {
                if client_takeover {
                    return Err(DeflateExtensionError::malformatted());
                } else {
                    client_takeover = true;
                    config.decompress_reset = true;
                    response_str.push_str("; client_no_context_takeover");
                }
            }
            param if param.starts_with("server_max_window_bits") => {
                if server_max_bits {
                    return Err(DeflateExtensionError::malformatted());
                } else {
                    server_max_bits = true;

                    let mut window_param = param.split("=").skip(1);
                    match window_param.next() {
                        Some(window_param) => {
                            if let Some(bits) =
                                parse_window_parameter(window_param, config.server_max_window_bits)?
                            {
                                config.server_max_window_bits = bits;
                            }
                        }
                        None => {
                            // If the client specifies 'server_max_window_bits' then a value must
                            // be provided.
                            return Err(DeflateExtensionError::InvalidMaxWindowBits);
                        }
                    }
                }
            }
            param if param.starts_with("client_max_window_bits") => {
                if client_max_bits {
                    return Err(DeflateExtensionError::malformatted());
                } else {
                    client_max_bits = true;

                    let mut window_param = param.split("=").skip(1);
                    if let Some(window_param) = window_param.next() {
                        // Absence of this parameter in an extension negotiation offer indicates
                        // that the client can receive messages compressed using an LZ77 sliding
                        // window of up to 32,768 bytes.
                        if let Some(bits) =
                            parse_window_parameter(window_param, config.client_max_window_bits)?
                        {
                            config.client_max_window_bits = bits;
                        }
                    }

                    response_str.push_str("; ");
                    response_str.push_str(&format!(
                        "client_max_window_bits={}",
                        config.client_max_window_bits()
                    ))
                }
            }
            p => {
                return Err(DeflateExtensionError::NegotiationError(
                    format!("Unknown permessage-deflate parameter: {}", p).into(),
                ))
            }
        }
    }

    if !response_str.contains("client_no_context_takeover") && config.request_no_context_takeover()
    {
        config.decompress_reset = true;
        response_str.push_str("; client_no_context_takeover");
    }

    if !response_str.contains("server_max_window_bits") {
        response_str.push_str("; ");
        response_str.push_str(&format!(
            "server_max_window_bits={}",
            config.server_max_window_bits()
        ))
    }

    if !response_str.contains("client_max_window_bits")
        && config.client_max_window_bits() < LZ77_MAX_WINDOW_SIZE
    {
        return Ok(None);
    }

    Ok(Some(response_str))
}

/// Verifies any required Sec-WebSocket-Extension headers in the HTTP request and updates the
/// response. Returns `Ok(true)` if a configuration could be agreed, `Ok(false)` if the HTTP header
/// was well formatted but no configuration could be agreed, or an error if it was malformatted.
pub fn on_receive_request<T>(
    request: &Request<T>,
    response: &mut Response<T>,
    config: &mut DeflateConfig,
) -> Result<bool, DeflateExtensionError> {
    for header in request.headers().get_all(SEC_WEBSOCKET_EXTENSIONS) {
        return match header.to_str() {
            Ok(header) => {
                for header in header.split(',') {
                    let mut parser_config = config.clone();

                    match validate_req_extensions(header, &mut parser_config) {
                        Ok(Some(response_str)) => {
                            response.headers_mut().insert(
                                SEC_WEBSOCKET_EXTENSIONS,
                                HeaderValue::from_str(&response_str)?,
                            );

                            *config = parser_config;
                            return Ok(true);
                        }
                        Ok(None) => continue,
                        Err(e) => {
                            response.headers_mut().remove(EXT_IDENT);
                            return Err(e);
                        }
                    }
                }
                Ok(false)
            }
            Err(e) => Err(DeflateExtensionError::NegotiationError(format!(
                "Failed to parse request header: {}",
                e,
            ))),
        };
    }

    Ok(false)
}

impl std::error::Error for DeflateExtensionError {}

impl From<DeflateExtensionError> for crate::Error {
    fn from(e: DeflateExtensionError) -> Self {
        crate::Error::ExtensionError(Cow::from(e.to_string()))
    }
}

impl From<InvalidHeaderValue> for DeflateExtensionError {
    fn from(e: InvalidHeaderValue) -> Self {
        DeflateExtensionError::NegotiationError(e.to_string())
    }
}

impl WebSocketExtension for DeflateExt {
    fn on_send_frame(&mut self, mut frame: Frame) -> Result<Frame, crate::Error> {
        if let OpCode::Data(_) = frame.header().opcode {
            let mut compressed = Vec::with_capacity(frame.payload().len());
            self.deflator.compress(frame.payload(), &mut compressed)?;

            let len = compressed.len();
            compressed.truncate(len - 4);

            *frame.payload_mut() = compressed;
            frame.header_mut().ext_headers.rsv1 = true;

            if self.config.compress_reset() {
                self.deflator.reset();
            }
        }

        Ok(frame)
    }

    fn on_receive_frame(
        &mut self,
        data_opcode: Data,
        is_final: bool,
        header: ExtensionHeaders,
        payload: Vec<u8>,
    ) -> Result<Option<Message>, crate::Error> {
        if !self.fragment_buffer.is_empty() || header.rsv1 {
            if !is_final {
                self.fragment_buffer.try_push(data_opcode, payload)?;
                Ok(None)
            } else {
                let mut compressed = if self.fragment_buffer.is_empty() {
                    Vec::with_capacity(payload.len())
                } else {
                    Vec::with_capacity(self.fragment_buffer.len() + payload.len())
                };

                let mut decompressed = Vec::with_capacity(payload.len() * 2);

                let message_type = match data_opcode {
                    Data::Continue => {
                        self.fragment_buffer.try_push(data_opcode, payload)?;
                        let (opcode, payload) = self.fragment_buffer.reset();

                        compressed = payload;
                        opcode
                    }
                    Data::Binary => {
                        compressed.put_slice(payload.as_slice());
                        IncompleteMessageType::Binary
                    }
                    Data::Text => {
                        compressed.put_slice(payload.as_slice());
                        IncompleteMessageType::Text
                    }
                    Data::Reserved(_) => {
                        return Err(crate::Error::ExtensionError(
                            "Unexpected reserved frame received".into(),
                        ))
                    }
                };

                compressed.extend(&[0, 0, 255, 255]);

                self.inflator.decompress(&compressed, &mut decompressed)?;

                if self.config.decompress_reset() {
                    self.inflator.reset(false);
                }

                let mut msg = IncompleteMessage::new(message_type);
                msg.extend(decompressed.as_slice(), self.config.max_message_size)?;

                Ok(Some(msg.complete()?))
            }
        } else {
            self.uncompressed_extension
                .on_receive_frame(data_opcode, is_final, header, payload)
        }
    }
}

impl From<DecompressError> for crate::Error {
    fn from(e: DecompressError) -> Self {
        crate::Error::ExtensionError(e.to_string().into())
    }
}

impl From<CompressError> for crate::Error {
    fn from(e: CompressError) -> Self {
        crate::Error::ExtensionError(e.to_string().into())
    }
}

#[derive(Debug)]
struct Deflator {
    compress: Compress,
}

impl Deflator {
    fn new(compression: Compression, mut window_size: u8) -> Deflator {
        // https://github.com/madler/zlib/blob/cacf7f1d4e3d44d871b605da3b647f07d718623f/deflate.c#L303
        if window_size == 8 {
            window_size = 9;
        }

        Deflator {
            compress: Compress::new_with_window_bits(compression, false, window_size),
        }
    }

    fn reset(&mut self) {
        self.compress.reset()
    }

    fn compress(&mut self, input: &[u8], output: &mut Vec<u8>) -> Result<(), CompressError> {
        let mut read_buff = Vec::from(input);
        let mut output_size;

        loop {
            output_size = output.len();

            if output_size == output.capacity() {
                output.reserve(input.len());
            }

            let before_out = self.compress.total_out();
            let before_in = self.compress.total_in();

            let out_slice = unsafe {
                slice::from_raw_parts_mut(
                    output.as_mut_ptr().offset(output_size as isize),
                    output.capacity() - output_size,
                )
            };

            let status = self
                .compress
                .compress(&read_buff, out_slice, FlushCompress::Sync)?;

            let consumed = (self.compress.total_in() - before_in) as usize;
            read_buff = read_buff.split_off(consumed);

            unsafe {
                output.set_len((self.compress.total_out() - before_out) as usize + output_size);
            }

            match status {
                Status::Ok | Status::BufError => {
                    if before_out == self.compress.total_out() && read_buff.is_empty() {
                        return Ok(());
                    }
                }
                s => panic!("Compression error: {:?}", s),
            }
        }
    }
}

#[derive(Debug)]
struct Inflator {
    decompress: Decompress,
}

impl Inflator {
    fn new(mut window_size: u8) -> Inflator {
        // https://github.com/madler/zlib/blob/cacf7f1d4e3d44d871b605da3b647f07d718623f/deflate.c#L303
        if window_size == 8 {
            window_size = 9;
        }

        Inflator {
            decompress: Decompress::new_with_window_bits(false, window_size),
        }
    }

    fn reset(&mut self, zlib_header: bool) {
        self.decompress.reset(zlib_header)
    }

    fn decompress(&mut self, input: &[u8], output: &mut Vec<u8>) -> Result<(), DecompressError> {
        let mut read_buff = Vec::from(input);
        let mut output_size;

        loop {
            output_size = output.len();

            if output_size == output.capacity() {
                output.reserve(input.len());
            }

            let before_out = self.decompress.total_out();
            let before_in = self.decompress.total_in();

            let out_slice = unsafe {
                slice::from_raw_parts_mut(
                    output.as_mut_ptr().offset(output_size as isize),
                    output.capacity() - output_size,
                )
            };

            let status =
                self.decompress
                    .decompress(&read_buff, out_slice, FlushDecompress::Sync)?;

            let consumed = (self.decompress.total_in() - before_in) as usize;
            read_buff = read_buff.split_off(consumed);

            unsafe {
                output.set_len((self.decompress.total_out() - before_out) as usize + output_size);
            }

            match status {
                Status::Ok | Status::BufError => {
                    if before_out == self.decompress.total_out() && read_buff.is_empty() {
                        return Ok(());
                    }
                }
                s => panic!("Decompression error: {:?}", s),
            }
        }
    }
}

/// A buffer for holding continuation frames. Ensures that the total length of all of the frame's
/// payloads does not exceed `max_len`.
///
/// Defaults to an initial capacity of ten frames.
#[derive(Debug)]
struct FragmentBuffer {
    frame_opcode: Option<IncompleteMessageType>,
    fragments: Vec<u8>,
    max_len: Option<usize>,
}

impl FragmentBuffer {
    /// Creates a new fragment buffer that will permit a maximum length of `max_len`.
    fn new(max_len: Option<usize>) -> FragmentBuffer {
        FragmentBuffer {
            frame_opcode: None,
            fragments: Vec::new(),
            max_len,
        }
    }

    /// Attempts to push a frame into the buffer. This will fail if the new length of the buffer's
    /// frames exceeds the maximum capacity of `max_len`.
    fn try_push(&mut self, opcode: Data, payload: Vec<u8>) -> Result<(), DeflateExtensionError> {
        let FragmentBuffer {
            fragments,
            max_len,
            frame_opcode,
        } = self;

        if fragments.is_empty() {
            let ty = match opcode {
                Data::Text => IncompleteMessageType::Text,
                Data::Binary => IncompleteMessageType::Binary,
                opc => {
                    return Err(DeflateExtensionError::Capacity(
                        format!("Expected a text or binary frame but received: {}", opc).into(),
                    ))
                }
            };

            *frame_opcode = Some(ty);
        }

        match max_len {
            Some(max_len) => {
                let mut fragments_len = fragments.len();
                fragments_len += payload.len();

                if fragments_len > *max_len || payload.len() > *max_len - fragments_len {
                    return Err(DeflateExtensionError::Capacity(
                        format!(
                            "Message too big: {} + {} > {}",
                            fragments_len, fragments_len, max_len
                        )
                        .into(),
                    ));
                } else {
                    fragments.extend(payload);
                    Ok(())
                }
            }
            None => {
                fragments.extend(payload);
                Ok(())
            }
        }
    }

    /// Returns the total length of all of the payloads that have been pushed into the buffer.
    fn len(&self) -> usize {
        self.fragments.len()
    }

    /// Returns whether the buffer is empty.
    fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }

    /// Drains the buffer. Returning the message's opcode and its payload.
    fn reset(&mut self) -> (IncompleteMessageType, Vec<u8>) {
        let payloads = replace(&mut self.fragments, Vec::new());
        (
            self.frame_opcode
                .take()
                .expect("Inconsistent state: missing opcode"),
            payloads,
        )
    }
}
