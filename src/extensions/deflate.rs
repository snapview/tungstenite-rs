//! Permessage-deflate extension

use std::fmt::{Display, Formatter};

use crate::extensions::WebSocketExtension;
use crate::protocol::frame::coding::{Data, OpCode};
use crate::protocol::frame::Frame;
use flate2::{Compress, CompressError, Compression, Decompress, DecompressError};
use std::mem::replace;

pub struct DeflateExtension {
    pub(crate) config: DeflateConfig,
    pub(crate) fragments: Vec<Frame>,
    inflator: Inflator,
    deflator: Deflator,
}

impl DeflateExtension {
    pub fn new() -> DeflateExtension {
        DeflateExtension {
            config: Default::default(),
            fragments: vec![],
            inflator: Inflator::new(),
            deflator: Deflator::new(Compression::best()),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DeflateConfig {
    /// The max size of the sliding window. If the other endpoint selects a smaller size, that size
    /// will be used instead. This must be an integer between 9 and 15 inclusive.
    /// Default: 15
    pub max_window_bits: u8,
    /// Indicates whether to ask the other endpoint to reset the sliding window for each message.
    /// Default: false
    pub request_no_context_takeover: bool,
    /// Indicates whether this endpoint will agree to reset the sliding window for each message it
    /// compresses. If this endpoint won't agree to reset the sliding window, then the handshake
    /// will fail if this endpoint is a client and the server requests no context takeover.
    /// Default: true
    pub accept_no_context_takeover: bool,
    /// The number of WebSocket frames to store when defragmenting an incoming fragmented
    /// compressed message.
    /// This setting may be different from the `fragments_capacity` setting of the WebSocket in order to
    /// allow for differences between compressed and uncompressed messages.
    /// Default: 10
    pub fragments_capacity: usize,
    /// Indicates whether the extension handler will reallocate if the `fragments_capacity` is
    /// exceeded. If this is not true, a capacity error will be triggered instead.
    /// Default: true
    pub fragments_grow: bool,
    compress_reset: bool,
    decompress_reset: bool,
}

impl Default for DeflateConfig {
    fn default() -> Self {
        DeflateConfig {
            max_window_bits: 15,
            request_no_context_takeover: false,
            accept_no_context_takeover: true,
            fragments_capacity: 10,
            fragments_grow: true,
            compress_reset: false,
            decompress_reset: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum DeflateExtensionError {
    DeflateError(String),
    InflateError(String),
}

impl Display for DeflateExtensionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DeflateExtensionError::DeflateError(m) => write!(f, "{}", m),
            DeflateExtensionError::InflateError(m) => write!(f, "{}", m),
        }
    }
}

impl std::error::Error for DeflateExtensionError {}

impl From<DeflateExtensionError> for crate::Error {
    fn from(e: DeflateExtensionError) -> Self {
        crate::Error::ExtensionError(Box::new(e))
    }
}

impl WebSocketExtension for DeflateExtension {
    type Error = DeflateExtensionError;

    fn on_send_frame(&mut self, mut frame: Frame) -> Result<Frame, Self::Error> {
        if let OpCode::Data(_) = frame.header().opcode {
            frame.header_mut().rsv1 = true;

            let mut compressed = Vec::with_capacity(frame.payload().len());
            self.deflator.compress(frame.payload(), &mut compressed)?;

            let len = compressed.len();
            compressed.truncate(len - 4);

            *frame.payload_mut() = compressed;

            if self.config.compress_reset {
                self.deflator.reset();
            }
        }

        Ok(frame)
    }

    fn on_receive_frame(&mut self, mut frame: Frame) -> Result<Option<Frame>, Self::Error> {
        if frame.header().rsv1 {
            frame.header_mut().rsv1 = false;

            if !frame.header().is_final {
                self.fragments.push(frame);
                return Ok(None);
            } else {
                if let OpCode::Data(Data::Continue) = frame.header().opcode {
                    if !self.config.fragments_grow
                        && self.config.fragments_capacity == self.fragments.len()
                    {
                        return Err(DeflateExtensionError::DeflateError(
                            "Exceeded max fragments.".into(),
                        ));
                    } else {
                        self.fragments.push(frame);
                    }

                    let opcode = self.fragments.first().unwrap().header().opcode;
                    let size = self
                        .fragments
                        .iter()
                        .fold(0, |len, frame| len + frame.payload().len());
                    let mut compressed = Vec::with_capacity(size);
                    let decompressed = Vec::with_capacity(size * 2);

                    replace(
                        &mut self.fragments,
                        Vec::with_capacity(self.config.fragments_capacity),
                    )
                    .into_iter()
                    .for_each(|f| {
                        compressed.extend(f.into_data());
                    });

                    compressed.extend(&[0, 0, 255, 255]);
                    frame = Frame::message(decompressed, opcode, true);
                } else {
                    frame.payload_mut().extend(&[0, 0, 255, 255]);

                    let mut decompress_output = Vec::with_capacity(frame.payload().len() * 2);
                    self.inflator
                        .decompress(frame.payload(), &mut decompress_output)?;

                    *frame.payload_mut() = decompress_output;
                }

                if self.config.decompress_reset {
                    self.inflator.reset(false);
                }
            }
        }

        Ok(Some(frame))
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

    pub fn compress(&mut self, input: &[u8], output: &mut Vec<u8>) -> Result<usize, CompressError> {
        loop {
            let before_in = self.compress.total_in();
            output.reserve(256);
            let status = self
                .compress
                .compress_vec(input, output, flate2::FlushCompress::Sync)?;
            let written = (self.compress.total_in() - before_in) as usize;

            if written != 0 || status == flate2::Status::StreamEnd {
                return Ok(written);
            }
        }
    }
}

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
    ) -> Result<usize, DecompressError> {
        let mut read_buff = Vec::from(input);
        let mut eof = false;

        loop {
            if read_buff.is_empty() {
                eof = true;
            }

            if !eof && output.is_empty() {
                output.reserve(256);

                unsafe {
                    output.set_len(output.capacity());
                }
            }

            let before_out = self.decompress.total_out();
            let before_in = self.decompress.total_in();

            let decompression_strategy = if eof {
                flate2::FlushDecompress::Finish
            } else {
                flate2::FlushDecompress::None
            };

            let status = self
                .decompress
                .decompress(&read_buff, output, decompression_strategy)?;

            let consumed = (self.decompress.total_in() - before_in) as usize;
            read_buff = read_buff.split_off(consumed);

            let read = (self.decompress.total_out() - before_out) as usize;

            if read != 0 || status == flate2::Status::StreamEnd {
                output.truncate(read);
                return Ok(read);
            }
        }
    }
}
