//! Permessage-deflate extension

use std::fmt::{Display, Formatter};

use crate::extensions::uncompressed::PlainTextExt;
use crate::extensions::WebSocketExtension;
use crate::protocol::frame::coding::{Data, OpCode};
use crate::protocol::frame::Frame;
use crate::protocol::message::{IncompleteMessage, IncompleteMessageType};
use crate::protocol::MAX_MESSAGE_SIZE;
use crate::{Error, Message};
use flate2::{
    Compress, CompressError, Compression, Decompress, DecompressError, FlushCompress,
    FlushDecompress, Status,
};
use http::header::{InvalidHeaderValue, SEC_WEBSOCKET_EXTENSIONS};
use http::{HeaderValue, Request, Response};
use std::mem::replace;
use std::slice;

const EXT_NAME: &str = "permessage-deflate";

/// A permessage-deflate configuration.
#[derive(Clone, Copy, Debug)]
pub struct DeflateConfig {
    /// The maximum size of a message. `None` means no size limit. The default value is 64 MiB
    /// which should be reasonably big for all normal use-cases but small enough to prevent
    /// memory eating by a malicious user.
    max_message_size: Option<usize>,
    /// The LZ77 sliding window size. Negotiated during the HTTP upgrade. In client mode, this
    /// conforms to RFC 7692 7.1.2.1. In server mode, this conforms to RFC 7692 7.1.2.2. Must be in
    /// range 9..=15.
    max_window_bits: u8,
    /// Request that the server resets the LZ77 sliding window between messages - RFC 7692 7.1.1.1.
    request_no_context_takeover: bool,
    accept_no_context_takeover: bool,
    compress_reset: bool,
    decompress_reset: bool,
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

    /// Returns the maximum LZ77 window size permitted.
    pub fn max_window_bits(&self) -> u8 {
        self.max_window_bits
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
        assert!((9u8..=15u8).contains(&max_window_bits));
        self.max_window_bits = max_window_bits;
    }

    /// Sets the WebSocket to request `no_context_takeover` if `true`.
    pub fn set_request_no_context_takeover(&mut self, request_no_context_takeover: bool) {
        self.request_no_context_takeover = request_no_context_takeover;
    }

    /// Sets the WebSocket to accept `no_context_takeover` if `true`.
    pub fn set_accept_no_context_takeover(&mut self, accept_no_context_takeover: bool) {
        self.accept_no_context_takeover = accept_no_context_takeover;
    }
}

impl Default for DeflateConfig {
    fn default() -> Self {
        DeflateConfig {
            max_message_size: Some(MAX_MESSAGE_SIZE),
            max_window_bits: 15,
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
    max_window_bits: u8,
    request_no_context_takeover: bool,
    accept_no_context_takeover: bool,
    fragments_grow: bool,
    compression_level: Compression,
}

impl Default for DeflateConfigBuilder {
    fn default() -> Self {
        DeflateConfigBuilder {
            max_message_size: Some(MAX_MESSAGE_SIZE),
            max_window_bits: 15,
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

    /// Sets the LZ77 sliding window size. Panics if the provided size is not in `9..=15`.
    pub fn max_window_bits(mut self, max_window_bits: u8) -> DeflateConfigBuilder {
        assert!(
            (9u8..=15u8).contains(&max_window_bits),
            "max window bits must be in range 9..=15"
        );
        self.max_window_bits = max_window_bits;
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
            max_window_bits: self.max_window_bits,
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
    /// Defines whether the extension is enabled. Following a successful handshake, this will be
    /// `true`.
    enabled: bool,
    /// The configuration for the extension.
    config: DeflateConfig,
    /// A stack of continuation frames awaiting `fin`.
    fragments: Vec<Frame>,
    /// The deflate decompressor.
    inflator: Inflator,
    /// The deflate compressor.
    deflator: Deflator,
    /// If this deflate extension is not used, messages will be forwarded to this extension.
    uncompressed_extension: PlainTextExt,
}

impl Clone for DeflateExt {
    fn clone(&self) -> Self {
        DeflateExt {
            enabled: self.enabled,
            config: self.config,
            fragments: vec![],
            inflator: Inflator::new(),
            deflator: Deflator::new(self.config.compression_level()),
            uncompressed_extension: PlainTextExt::new(self.config.max_message_size()),
        }
    }
}

impl Default for DeflateExt {
    fn default() -> Self {
        DeflateExt::new(Default::default())
    }
}

impl DeflateExt {
    /// Creates a `DeflateExt` instance using the provided configuration.
    pub fn new(config: DeflateConfig) -> DeflateExt {
        DeflateExt {
            enabled: false,
            config,
            fragments: vec![],
            inflator: Inflator::new(),
            deflator: Deflator::new(Compression::fast()),
            uncompressed_extension: PlainTextExt::new(config.max_message_size()),
        }
    }

    fn complete_message(&self, data: Vec<u8>, opcode: OpCode) -> Result<Message, Error> {
        let message_type = match opcode {
            OpCode::Data(Data::Text) => IncompleteMessageType::Text,
            OpCode::Data(Data::Binary) => IncompleteMessageType::Binary,
            _ => panic!("Bug: message is not text nor binary"),
        };

        let mut incomplete_message = IncompleteMessage::new(message_type);
        incomplete_message.extend(data, self.config.max_message_size())?;
        incomplete_message.complete()
    }

    fn parse_window_parameter<'a>(
        &self,
        mut param_iter: impl Iterator<Item = &'a str>,
    ) -> Result<Option<u8>, String> {
        if let Some(window_bits_str) = param_iter.next() {
            match window_bits_str.trim().parse() {
                Ok(mut window_bits) => {
                    if window_bits == 8 {
                        window_bits = 9;
                    }

                    if window_bits >= 9 && window_bits <= 15 {
                        if window_bits != self.config.max_window_bits() {
                            Ok(Some(window_bits))
                        } else {
                            Ok(None)
                        }
                    } else {
                        Err(format!("Invalid window parameter: {}", window_bits))
                    }
                }
                Err(e) => Err(e.to_string()),
            }
        } else {
            Ok(None)
        }
    }

    fn decline<T>(&mut self, res: &mut Response<T>) {
        self.enabled = false;
        res.headers_mut().remove(EXT_NAME);
    }
}

/// A permessage-deflate extension error.
#[derive(Debug, Clone)]
pub enum DeflateExtensionError {
    /// An error produced when deflating a message.
    DeflateError(String),
    /// An error produced when inflating a message.
    InflateError(String),
    /// An error produced during the WebSocket negotiation.
    NegotiationError(String),
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
        }
    }
}

impl std::error::Error for DeflateExtensionError {}

impl From<DeflateExtensionError> for crate::Error {
    fn from(e: DeflateExtensionError) -> Self {
        crate::Error::ExtensionError(Box::new(e))
    }
}

impl From<InvalidHeaderValue> for DeflateExtensionError {
    fn from(e: InvalidHeaderValue) -> Self {
        DeflateExtensionError::NegotiationError(e.to_string())
    }
}

impl WebSocketExtension for DeflateExt {
    type Error = DeflateExtensionError;

    fn new(max_message_size: Option<usize>) -> Self {
        DeflateExt::new(DeflateConfig {
            max_message_size,
            ..Default::default()
        })
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn on_make_request<T>(&mut self, mut request: Request<T>) -> Request<T> {
        let mut header_value = String::from(EXT_NAME);
        let DeflateConfig {
            max_window_bits,
            request_no_context_takeover,
            ..
        } = self.config;

        if max_window_bits < 15 {
            header_value.push_str(&format!(
                "; client_max_window_bits={}; server_max_window_bits={}",
                max_window_bits, max_window_bits
            ))
        } else {
            header_value.push_str("; client_max_window_bits")
        }

        if request_no_context_takeover {
            header_value.push_str("; server_no_context_takeover")
        }

        request.headers_mut().append(
            SEC_WEBSOCKET_EXTENSIONS,
            HeaderValue::from_str(&header_value).unwrap(),
        );

        request
    }

    fn on_receive_request<T>(
        &mut self,
        request: &Request<T>,
        response: &mut Response<T>,
    ) -> Result<(), Self::Error> {
        for header in request.headers().get_all(SEC_WEBSOCKET_EXTENSIONS) {
            return match header.to_str() {
                Ok(header) => {
                    let mut response_str = String::with_capacity(header.len());
                    let mut server_takeover = false;
                    let mut client_takeover = false;
                    let mut server_max_bits = false;
                    let mut client_max_bits = false;

                    for param in header.split(';') {
                        match param.trim() {
                            "permessage-deflate" => response_str.push_str("permessage-deflate"),
                            "server_no_context_takeover" => {
                                if server_takeover {
                                    self.decline(response);
                                } else {
                                    server_takeover = true;
                                    if self.config.accept_no_context_takeover() {
                                        self.config.compress_reset = true;
                                        response_str.push_str("; server_no_context_takeover");
                                    }
                                }
                            }
                            "client_no_context_takeover" => {
                                if client_takeover {
                                    self.decline(response);
                                } else {
                                    client_takeover = true;
                                    self.config.decompress_reset = true;
                                    response_str.push_str("; client_no_context_takeover");
                                }
                            }
                            param if param.starts_with("server_max_window_bits") => {
                                if server_max_bits {
                                    self.decline(response);
                                } else {
                                    server_max_bits = true;

                                    match self.parse_window_parameter(param.split('=').skip(1)) {
                                        Ok(Some(bits)) => {
                                            self.deflator = Deflator {
                                                compress: Compress::new_with_window_bits(
                                                    self.config.compression_level(),
                                                    false,
                                                    bits,
                                                ),
                                            };
                                            response_str.push_str("; ");
                                            response_str.push_str(param)
                                        }
                                        Ok(None) => {}
                                        Err(_) => {
                                            self.decline(response);
                                        }
                                    }
                                }
                            }
                            param if param.starts_with("client_max_window_bits") => {
                                if client_max_bits {
                                    self.decline(response);
                                } else {
                                    client_max_bits = true;

                                    match self.parse_window_parameter(param.split('=').skip(1)) {
                                        Ok(Some(bits)) => {
                                            self.inflator = Inflator {
                                                decompress: Decompress::new_with_window_bits(
                                                    false, bits,
                                                ),
                                            };
                                            response_str.push_str("; ");
                                            response_str.push_str(param);
                                            continue;
                                        }
                                        Ok(None) => {}
                                        Err(_) => {
                                            self.decline(response);
                                        }
                                    }

                                    response_str.push_str("; ");
                                    response_str.push_str(&format!(
                                        "client_max_window_bits={}",
                                        self.config.max_window_bits()
                                    ))
                                }
                            }
                            _ => {
                                self.decline(response);
                            }
                        }
                    }

                    if !response_str.contains("client_no_context_takeover")
                        && self.config.request_no_context_takeover()
                    {
                        self.config.decompress_reset = true;
                        response_str.push_str("; client_no_context_takeover");
                    }

                    if !response_str.contains("server_max_window_bits") {
                        response_str.push_str("; ");
                        response_str.push_str(&format!(
                            "server_max_window_bits={}",
                            self.config.max_window_bits()
                        ))
                    }

                    if !response_str.contains("client_max_window_bits")
                        && self.config.max_window_bits() < 15
                    {
                        continue;
                    }

                    response.headers_mut().insert(
                        SEC_WEBSOCKET_EXTENSIONS,
                        HeaderValue::from_str(&response_str)?,
                    );

                    self.enabled = true;

                    Ok(())
                }
                Err(e) => {
                    self.enabled = false;
                    Err(DeflateExtensionError::NegotiationError(format!(
                        "Failed to parse request header: {}",
                        e,
                    )))
                }
            };
        }

        self.decline(response);
        Ok(())
    }

    fn on_response<T>(&mut self, response: &Response<T>) -> Result<(), Self::Error> {
        let mut extension_name = false;
        let mut server_takeover = false;
        let mut client_takeover = false;
        let mut server_max_window_bits = false;
        let mut client_max_window_bits = false;

        for header in response.headers().get_all(SEC_WEBSOCKET_EXTENSIONS).iter() {
            match header.to_str() {
                Ok(header) => {
                    for param in header.split(';') {
                        match param.trim() {
                            "permessage-deflate" => {
                                if extension_name {
                                    return Err(DeflateExtensionError::NegotiationError(format!(
                                        "Duplicate extension parameter permessage-deflate"
                                    )));
                                } else {
                                    self.enabled = true;
                                    extension_name = true;
                                }
                            }
                            "server_no_context_takeover" => {
                                if server_takeover {
                                    return Err(DeflateExtensionError::NegotiationError(format!(
                                        "Duplicate extension parameter server_no_context_takeover"
                                    )));
                                } else {
                                    server_takeover = true;
                                    self.config.decompress_reset = true;
                                }
                            }
                            "client_no_context_takeover" => {
                                if client_takeover {
                                    return Err(DeflateExtensionError::NegotiationError(format!(
                                        "Duplicate extension parameter client_no_context_takeover"
                                    )));
                                } else {
                                    client_takeover = true;

                                    if self.config.accept_no_context_takeover() {
                                        self.config.compress_reset = true;
                                    } else {
                                        return Err(DeflateExtensionError::NegotiationError(
                                            format!("The client requires context takeover."),
                                        ));
                                    }
                                }
                            }
                            param if param.starts_with("server_max_window_bits") => {
                                if server_max_window_bits {
                                    return Err(DeflateExtensionError::NegotiationError(format!(
                                        "Duplicate extension parameter server_max_window_bits"
                                    )));
                                } else {
                                    server_max_window_bits = true;

                                    match self.parse_window_parameter(param.split("=").skip(1)) {
                                        Ok(Some(bits)) => {
                                            self.deflator = Deflator {
                                                compress: Compress::new_with_window_bits(
                                                    self.config.compression_level(),
                                                    false,
                                                    bits,
                                                ),
                                            };
                                        }
                                        Ok(None) => {}
                                        Err(e) => {
                                            return Err(DeflateExtensionError::NegotiationError(
                                                format!(
                                                    "server_max_window_bits parameter error: {}",
                                                    e
                                                ),
                                            ))
                                        }
                                    }
                                }
                            }
                            param if param.starts_with("client_max_window_bits") => {
                                if client_max_window_bits {
                                    return Err(DeflateExtensionError::NegotiationError(format!(
                                        "Duplicate extension parameter client_max_window_bits"
                                    )));
                                } else {
                                    client_max_window_bits = true;

                                    match self.parse_window_parameter(param.split("=").skip(1)) {
                                        Ok(Some(bits)) => {
                                            self.inflator = Inflator {
                                                decompress: Decompress::new_with_window_bits(
                                                    false, bits,
                                                ),
                                            };
                                        }
                                        Ok(None) => {}
                                        Err(e) => {
                                            return Err(DeflateExtensionError::NegotiationError(
                                                format!(
                                                    "client_max_window_bits parameter error: {}",
                                                    e
                                                ),
                                            ))
                                        }
                                    }
                                }
                            }
                            param => {
                                return Err(DeflateExtensionError::NegotiationError(format!(
                                    "Unknown permessage-deflate parameter: {}",
                                    param
                                )));
                            }
                        }
                    }
                }
                Err(e) => {
                    self.enabled = false;
                    return Err(DeflateExtensionError::NegotiationError(format!(
                        "Failed to parse extension parameter: {}",
                        e
                    )));
                }
            }
        }

        Ok(())
    }

    fn on_send_frame(&mut self, mut frame: Frame) -> Result<Frame, Self::Error> {
        if self.enabled {
            if let OpCode::Data(_) = frame.header().opcode {
                let mut compressed = Vec::with_capacity(frame.payload().len());
                self.deflator.compress(frame.payload(), &mut compressed)?;

                let len = compressed.len();
                compressed.truncate(len - 4);

                *frame.payload_mut() = compressed;
                frame.header_mut().rsv1 = true;

                if self.config.compress_reset() {
                    self.deflator.reset();
                }
            }
        }

        Ok(frame)
    }

    fn on_receive_frame(&mut self, mut frame: Frame) -> Result<Option<Message>, Self::Error> {
        match frame.header().opcode {
            OpCode::Control(_) => unreachable!(),
            _ => {
                if self.enabled && (!self.fragments.is_empty() || frame.header().rsv1) {
                    if !frame.header().is_final {
                        self.fragments.push(frame);
                        return Ok(None);
                    } else {
                        let message = if let OpCode::Data(Data::Continue) = frame.header().opcode {
                            self.fragments.push(frame);

                            let opcode = self.fragments.first().unwrap().header().opcode;
                            let size = self
                                .fragments
                                .iter()
                                .fold(0, |len, frame| len + frame.payload().len());
                            let mut compressed = Vec::with_capacity(size);
                            let mut decompressed = Vec::with_capacity(size * 2);

                            replace(&mut self.fragments, Vec::with_capacity(10))
                                .into_iter()
                                .for_each(|f| {
                                    compressed.extend(f.into_data());
                                });

                            compressed.extend(&[0, 0, 255, 255]);

                            self.inflator.decompress(&compressed, &mut decompressed)?;

                            self.complete_message(decompressed, opcode)
                        } else {
                            frame.payload_mut().extend(&[0, 0, 255, 255]);
                            let mut decompressed = Vec::with_capacity(frame.payload().len() * 2);
                            self.inflator
                                .decompress(frame.payload(), &mut decompressed)?;

                            self.complete_message(decompressed, frame.header().opcode)
                        };

                        if self.config.decompress_reset() {
                            self.inflator.reset(false);
                        }

                        match message {
                            Ok(message) => Ok(Some(message)),
                            Err(e) => Err(DeflateExtensionError::DeflateError(e.to_string())),
                        }
                    }
                } else {
                    self.uncompressed_extension
                        .on_receive_frame(frame)
                        .map_err(|e| DeflateExtensionError::DeflateError(e.to_string()))
                }
            }
        }
    }
}

impl From<DecompressError> for DeflateExtensionError {
    fn from(e: DecompressError) -> Self {
        DeflateExtensionError::InflateError(e.to_string())
    }
}

impl From<CompressError> for DeflateExtensionError {
    fn from(e: CompressError) -> Self {
        DeflateExtensionError::DeflateError(e.to_string())
    }
}

#[derive(Debug)]
struct Deflator {
    compress: Compress,
}

impl Deflator {
    pub fn new(compresion: Compression) -> Deflator {
        Deflator {
            compress: Compress::new(compresion, false),
        }
    }

    fn reset(&mut self) {
        self.compress.reset()
    }

    pub fn compress(&mut self, input: &[u8], output: &mut Vec<u8>) -> Result<(), CompressError> {
        let mut read_buff = Vec::from(input);
        let mut output_size;

        loop {
            output_size = output.len();

            if output_size == output.capacity() {
                output.reserve(input.len());
            }

            let before_out = self.compress.total_out();
            let before_in = self.compress.total_in();

            let status = self
                .compress
                .compress_vec(&read_buff, output, FlushCompress::Sync)?;

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
    pub fn new() -> Inflator {
        Inflator {
            decompress: Decompress::new(false),
        }
    }

    fn reset(&mut self, zlib_header: bool) {
        self.decompress.reset(zlib_header)
    }

    pub fn decompress(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<(), DecompressError> {
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
