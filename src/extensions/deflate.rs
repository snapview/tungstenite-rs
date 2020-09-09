//! Permessage-deflate extension

use std::fmt::{Display, Formatter};

use crate::extensions::WebSocketExtension;
use crate::protocol::frame::coding::{Data, OpCode};
use crate::protocol::frame::Frame;
use flate2::{
    Compress, CompressError, Compression, Decompress, DecompressError, FlushCompress,
    FlushDecompress, Status,
};
use std::mem::replace;
use std::slice;

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

            // println!("Compressing: {:?}", frame.payload());

            let mut compressed = Vec::with_capacity(frame.payload().len() * 2);
            self.deflator.compress(frame.payload(), &mut compressed)?;

            let len = compressed.len();
            compressed.truncate(len - 4);

            println!("Compressed to: {:?}", compressed.len());

            *frame.payload_mut() = compressed;

            if self.config.compress_reset {
                self.deflator.reset();
            }
        }

        Ok(frame)
    }

    fn on_receive_frame(&mut self, mut frame: Frame) -> Result<Option<Frame>, Self::Error> {
        match frame.header().opcode {
            OpCode::Control(_) => Ok(Some(frame)),
            _ => {
                if !self.fragments.is_empty() || frame.header().rsv1 {
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
                            let mut decompressed = Vec::with_capacity(size * 2);

                            replace(
                                &mut self.fragments,
                                Vec::with_capacity(self.config.fragments_capacity),
                            )
                            .into_iter()
                            .for_each(|f| {
                                compressed.extend(f.into_data());
                            });

                            compressed.extend(&[0, 0, 255, 255]);

                            self.inflator.decompress(&compressed, &mut decompressed)?;

                            frame = Frame::message(decompressed, opcode, true);
                        } else {
                            frame.payload_mut().extend(&[0, 0, 255, 255]);

                            let mut decompress_output =
                                Vec::with_capacity(frame.payload().len() * 2);
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

    // pub fn compress(&mut self, input: &[u8], output: &mut Vec<u8>) -> Result<usize, CompressError> {
    //     loop {
    //         let before_in = self.compress.total_in();
    //         output.reserve(256);
    //         let status = self
    //             .compress
    //             .compress_vec(input, output, flate2::FlushCompress::Sync)?;
    //         let written = (self.compress.total_in() - before_in) as usize;
    //
    //         if written != 0 || status == flate2::Status::StreamEnd {
    //             return Ok(written);
    //         }
    //     }
    // }

    pub fn compress(&mut self, input: &[u8], output: &mut Vec<u8>) -> Result<usize, CompressError> {
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

            let new_size = (self.compress.total_out() - before_out) as usize + output_size;

            unsafe {
                output.set_len(new_size);
            }

            match status {
                Status::Ok | Status::BufError => {
                    if before_out == self.compress.total_out() && read_buff.is_empty() {
                        return Ok(consumed);
                    }
                }
                s => panic!(s),
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
                        return Ok(consumed);
                    } else {
                        continue;
                    }
                }
                s => panic!(s),
            }
        }
    }
}

#[test]
fn t() {
    let v1 = vec![
        37, 80, 68, 70, 45, 49, 46, 50, 10, 37, 199, 236, 143, 162, 10, 54, 32, 48, 32, 111, 98,
        106, 10, 60, 60, 47, 76, 101, 110, 103, 116, 104, 32, 55, 32, 48, 32, 82, 47, 70, 105, 108,
        116, 101, 114, 32, 47, 70, 108, 97, 116, 101, 68, 101, 99, 111, 100, 101, 62, 62, 10, 115,
        116, 114, 101, 97, 109, 10, 120, 156, 125, 83, 193, 110, 212, 48, 16, 189, 231, 192, 55,
        248, 88, 36, 118, 106, 123, 198, 118, 124, 44, 168, 160, 10, 1, 106, 27, 46, 220, 178, 219,
        236, 174, 209, 38, 105, 55, 217, 138, 254, 61, 99, 39, 54, 55, 148, 67, 148, 55, 227, 241,
        123, 111, 94, 36, 40, 33, 227, 179, 190, 119, 125, 245, 82, 93, 63, 144, 56, 76, 213, 139,
        80, 169, 148, 95, 187, 94, 124, 108, 184, 200, 159, 202, 129, 86, 198, 136, 102, 95, 45,
        231, 148, 80, 74, 129, 247, 86, 88, 167, 193, 213, 74, 52, 125, 117, 117, 251, 110, 23,
        186, 97, 126, 223, 252, 174, 156, 132, 218, 58, 238, 108, 158, 170, 171, 38, 34, 220, 47,
        9, 253, 10, 157, 219, 48, 132, 225, 16, 11, 198, 131, 38, 107, 214, 194, 184, 143, 152, 86,
        96, 12, 230, 243, 63, 182, 83, 119, 126, 109, 183, 167, 46, 214, 188, 6, 35, 109, 157, 107,
        207, 169, 31, 161, 70, 149, 251, 187, 115, 59, 143, 17, 38, 214, 41, 203, 232, 115, 132,
        54, 154, 9, 123, 227, 197, 70, 27, 208, 104, 83, 229, 91, 234, 214, 22, 156, 215, 121, 240,
        83, 119, 154, 34, 138, 22, 208, 35, 173, 232, 101, 90, 89, 147, 5, 203, 221, 43, 252, 105,
        28, 230, 238, 79, 82, 110, 107, 168, 165, 202, 58, 191, 156, 219, 231, 99, 154, 195, 70,
        162, 80, 4, 72, 86, 71, 35, 55, 74, 19, 72, 75, 98, 227, 16, 8, 213, 63, 167, 106, 240, 78,
        187, 98, 200, 54, 180, 11, 17, 3, 158, 36, 230, 193, 144, 76, 245, 96, 139, 73, 43, 32, 61,
        81, 49, 98, 154, 187, 48, 36, 221, 150, 117, 96, 188, 142, 164, 101, 179, 86, 225, 45, 55,
        36, 95, 136, 87, 76, 70, 229, 141, 29, 187, 41, 100, 218, 150, 19, 1, 94, 122, 181, 208,
        38, 15, 198, 90, 158, 131, 96, 105, 161, 125, 55, 76, 115, 152, 47, 115, 218, 15, 17, 215,
        41, 27, 176, 31, 211, 120, 197, 118, 213, 42, 147, 191, 185, 204, 227, 48, 246, 227, 37,
        93, 97, 17, 60, 22, 189, 11, 221, 58, 134, 74, 231, 133, 206, 221, 233, 20, 14, 93, 170,
        32, 113, 22, 139, 13, 75, 216, 56, 107, 117, 238, 125, 124, 99, 73, 253, 244, 33, 81, 225,
        21, 27, 157, 151, 244, 57, 93, 6, 90, 162, 163, 18, 195, 203, 112, 28, 247, 171, 5, 158,
        55, 237, 242, 156, 155, 240, 184, 228, 197, 32, 24, 66, 177, 81, 53, 40, 52, 171, 109, 243,
        177, 235, 219, 57, 236, 218, 83, 26, 202, 39, 149, 207, 155, 248, 143, 27, 215, 41, 192,
        32, 157, 183, 217, 234, 95, 55, 119, 95, 19, 89, 100, 27, 56, 173, 43, 252, 115, 8, 175,
        41, 148, 252, 123, 169, 34, 129, 23, 26, 146, 100, 45, 1, 109, 249, 193, 222, 150, 177,
        200, 185, 118, 37, 145, 167, 241, 48, 36, 10, 156, 52, 13, 202, 241, 202, 120, 11, 44, 61,
        150, 191, 167, 196, 43, 4, 148, 117, 118, 62, 221, 103, 128, 76, 137, 79, 191, 54, 89, 93,
        20, 108, 23, 19, 209, 74, 83, 56, 165, 46, 230, 105, 40, 139, 210, 82, 234, 136, 222, 54,
        226, 190, 186, 175, 254, 2, 247, 54, 15, 175, 101, 110, 100, 115, 116, 114, 101, 97, 109,
        10, 101, 110, 100, 111, 98, 106, 10, 55, 32, 48, 32, 111, 98, 106, 10, 53, 55, 56, 10, 101,
        110, 100, 111, 98, 106, 10, 49, 55, 32, 48, 32, 111, 98, 106, 10, 60, 60, 47, 82, 52, 10,
        52, 32, 48, 32, 82, 62, 62, 10, 101, 110, 100, 111, 98, 106, 10, 49, 56, 32, 48, 32, 111,
        98, 106, 10, 60, 60, 47, 82, 49, 54, 10, 49, 54, 32, 48, 32, 82, 47, 82, 49, 51, 10, 49,
        51, 32, 48, 32, 82, 47, 82, 49, 48, 10, 49, 48, 32, 48, 32, 82, 62, 62, 10, 101, 110, 100,
        111, 98, 106, 10, 50, 51, 32, 48, 32, 111, 98, 106, 10, 60, 60, 47, 76, 101, 110, 103, 116,
        104, 32, 50, 52, 32, 48, 32, 82, 47, 70, 105, 108, 116, 101, 114, 32, 47, 70, 108, 97, 116,
        101, 68, 101, 99, 111, 100, 101, 62, 62, 10, 115, 116, 114, 101, 97, 109, 10, 120, 156,
        173, 86, 75, 143, 27, 69, 16, 190, 91, 252, 8, 31, 55, 82, 92, 116, 87, 191, 143, 139, 128,
        16, 16, 129, 100, 205, 5, 229, 50, 182, 219, 235, 129, 241, 12, 59, 211, 78, 216, 83, 254,
        58, 53, 253, 178, 29, 22, 177, 72, 200, 7, 75, 213, 221, 85, 213, 223, 163, 122, 24, 240,
        37, 155, 127, 249, 127, 123, 92, 60, 44, 30, 150, 60, 198, 202, 223, 246, 184, 252, 106,
        189, 248, 242, 29, 114, 138, 128, 99, 142, 47, 215, 251, 69, 58, 192, 151, 232, 12, 48, 52,
        75, 173, 20, 232, 229, 250, 184, 184, 185, 221, 189, 88, 255, 182, 224, 10, 164, 148, 130,
        246, 172, 119, 139, 155, 15, 237, 52, 140, 211, 28, 167, 60, 102, 233, 192, 105, 212, 114,
        206, 179, 226, 22, 193, 162, 94, 174, 148, 5, 235, 116, 220, 206, 33, 166, 64, 224, 218,
        230, 12, 183, 187, 152, 99, 142, 175, 242, 194, 138, 107, 48, 50, 174, 254, 60, 14, 251,
        120, 6, 21, 37, 81, 58, 31, 250, 122, 76, 137, 44, 104, 167, 100, 14, 190, 107, 218, 222,
        199, 68, 2, 1, 37,
    ];
    let v2 = vec![
        170, 28, 191, 219, 206, 49, 7, 218, 136, 90, 245, 48, 54, 187, 180, 119, 101, 53, 40, 102,
        48, 85, 21, 60, 46, 255, 234, 251, 124, 87, 163, 152, 203, 103, 194, 120, 58, 198, 86, 36,
        56, 44, 137, 246, 177, 30, 8, 110, 84, 9, 125, 138, 105, 25, 160, 149, 90, 150, 230, 78,
        99, 190, 185, 145, 186, 220, 226, 182, 191, 247, 31, 99, 70, 7, 232, 24, 207, 225, 166,
        223, 5, 31, 195, 6, 180, 174, 45, 191, 238, 247, 195, 120, 108, 66, 251, 251, 188, 38, 29,
        40, 94, 75, 254, 16, 107, 26, 234, 95, 137, 66, 205, 208, 245, 9, 84, 195, 40, 141, 185,
        68, 245, 151, 190, 253, 16, 11, 48, 48, 136, 229, 128, 31, 167, 54, 124, 202, 184, 90, 2,
        171, 244, 19, 10, 105, 86, 62, 167, 160, 154, 91, 35, 222, 133, 0, 105, 18, 158, 248, 95,
        121, 63, 19, 108, 88, 197, 240, 59, 63, 110, 98, 219, 28, 148, 115, 181, 235, 144, 169, 34,
        60, 74, 43, 223, 55, 254, 62, 211, 75, 242, 181, 220, 94, 93, 255, 117, 63, 133, 54, 156,
        226, 57, 161, 1, 141, 82, 255, 7, 155, 167, 48, 244, 195, 49, 50, 39, 45, 8, 235, 10, 161,
        109, 196, 197, 2, 99, 206, 149, 12, 193, 119, 93, 123, 159, 100, 38, 56, 32, 199, 42, 179,
        152, 129, 115, 208, 66, 151, 216, 221, 227, 20, 124, 74, 189, 226, 138, 131, 96, 242, 74,
        175, 223, 206, 43, 228, 83, 99, 116, 41, 58, 54, 167, 254, 48, 236, 19, 10, 82, 1, 39, 71,
        148, 78, 219, 187, 236, 216, 107, 231, 51, 13, 179, 164, 86, 142, 236, 163, 92, 226, 193,
        111, 187, 102, 108, 98, 14, 13, 66, 10, 83, 186, 108, 135, 254, 73, 219, 35, 19, 169, 189,
        11, 219, 175, 15, 109, 156, 17, 72, 19, 129, 155, 210, 70, 56, 248, 41, 199, 13, 88, 81,
        161, 73, 49, 106, 140, 217, 10, 192, 166, 153, 252, 46, 111, 53, 115, 218, 44, 185, 100,
        83, 1, 142, 243, 18, 139, 142, 210, 116, 43, 201, 10, 135, 195, 24, 61, 195, 13, 40, 89,
        46, 176, 109, 198, 177, 77, 57, 73, 168, 2, 109, 221, 156, 132, 65, 155, 141, 49, 252, 115,
        15, 160, 170, 10, 166, 254, 243, 70, 68, 135, 213, 165, 73, 91, 73, 6, 228, 47, 83, 149,
        186, 79, 106, 39, 165, 10, 212, 226, 90, 53, 195, 41, 94, 154, 48, 147, 6, 245, 19, 186,
        225, 255, 174, 155, 228, 72, 193, 207, 131, 239, 113, 90, 69, 201, 208, 220, 3, 205, 240,
        210, 3, 36, 167, 233, 101, 4, 84, 3, 141, 248, 130, 202, 115, 132, 164, 221, 133, 144, 94,
        102, 67, 90, 39, 75, 219, 175, 60, 141, 169, 212, 160, 132, 106, 143, 199, 120, 28, 36,
        205, 155, 146, 25, 114, 199, 146, 85, 56, 222, 12, 25, 34, 203, 108, 137, 253, 209, 36,
        139, 83, 21, 77, 30, 44, 52, 237, 179, 77, 132, 149, 21, 130, 172, 51, 238, 64, 158, 153,
        190, 210, 153, 113, 213, 196, 135, 102, 202, 252, 89, 86, 135, 252, 38, 1, 64, 229, 207,
        115, 49, 63, 6, 116, 88, 98, 233, 125, 58, 109, 142, 109, 8, 73, 64, 210, 64, 137, 251,
        110, 242, 31, 15, 126, 76, 252, 75, 16, 26, 241, 154, 255, 149, 48, 114, 30, 187, 87, 6,
        78, 120, 145, 230, 17, 249, 21, 98, 22, 184, 19, 149, 227, 129, 238, 50, 230, 231, 80, 11,
        83, 194, 59, 127, 63, 250, 88, 81, 208, 172, 231, 12, 171, 238, 51, 70, 150, 179, 130, 198,
        195, 169, 233, 218, 47, 182, 77, 49, 176, 162, 1, 104, 47, 158, 158, 60, 118, 157, 169, 69,
        155, 174, 203, 46, 147, 66, 60, 203, 101, 181, 216, 19, 187, 50, 232, 124, 118, 124, 73,
        182, 27, 250, 244, 224, 145, 17, 233, 225, 185, 166, 2, 213, 252, 25, 242, 143, 144, 28,
        255, 62, 91, 31, 39, 223, 237, 243, 188, 33, 113, 21, 52, 78, 125, 231, 167, 41, 191, 183,
        12, 207, 2, 247, 36, 110, 223, 111, 51, 151, 212, 174, 172, 158, 13, 67, 238, 246, 194,
        199, 217, 247, 196, 227, 60, 206, 244, 165, 171, 182, 121, 32, 145, 238, 101, 113, 97, 24,
        155, 241, 49, 115, 134, 88, 57, 75, 230, 158, 199, 60, 183, 159, 101, 38, 4, 185, 168, 38,
        11, 254, 207, 0, 233, 41, 35, 114, 221, 60, 89, 221, 60, 134, 210, 100, 77, 0, 11, 165,
        206, 52, 108, 218, 132, 49, 213, 147, 231, 9, 250, 10, 202, 168, 100, 188, 32, 252, 211,
        38, 199, 148, 172, 229, 232, 43, 32, 248, 212, 28, 125, 70, 113, 85, 129, 126, 127, 243,
        99, 19, 210, 179, 76, 11, 78, 212, 239, 149, 55, 245, 173, 86, 213, 157, 2, 37, 231, 136,
        239, 95, 204, 43, 223, 172, 151, 111, 23, 111, 23, 127, 1, 75, 50, 131, 211, 101, 110, 100,
        115, 116, 114, 101, 97, 109, 10, 101, 110, 100, 111, 98, 106, 10, 50, 52, 32, 48, 32, 111,
        98, 106, 10, 49, 48, 54, 56, 10, 101, 110, 100, 111, 98, 106, 10, 50, 56, 32, 48, 32, 111,
        98, 106, 10, 60, 60, 47, 82, 50, 55, 10, 50, 55, 32, 48, 32, 82, 47, 82, 50, 49, 10, 50,
        49, 32, 48, 32, 82, 62, 62, 10, 101, 110, 100, 111, 98, 106, 10, 51, 51, 32, 48, 32, 111,
        98, 106, 10, 60, 60, 47, 76, 101, 110, 103, 116, 104, 32, 51, 52, 32, 48, 32, 82, 47, 70,
        105, 108, 116, 101, 114, 32, 47, 70, 108, 97, 116, 101, 68, 101, 99, 111, 100, 101, 62, 62,
        10, 115, 116, 114, 101, 97, 109, 10, 120, 156, 205, 90, 77, 111, 28, 199, 17, 189, 211, 70,
        126, 3, 143, 10, 64, 78, 250, 187, 123, 114, 147,
    ];
    let v3 = vec![
        45, 195, 49, 18, 66, 182, 41, 36, 128, 161, 203, 112, 183, 197, 29, 115, 119, 134, 158,
        153, 149, 44, 93, 244, 215, 83, 61, 93, 85, 61, 187, 92, 146, 138, 147, 67, 160, 131, 128,
        222, 222, 238, 234, 170, 87, 175, 94, 213, 82, 84, 242, 92, 164, 127, 248, 255, 106, 119,
        246, 219, 217, 111, 231, 114, 94, 163, 255, 86, 187, 243, 111, 222, 156, 253, 229, 103, 45,
        207, 93, 85, 123, 29, 236, 249, 155, 119, 103, 249, 11, 242, 220, 56, 89, 185, 90, 158,
        123, 171, 170, 90, 195, 71, 187, 179, 23, 109, 219, 254, 249, 205, 175, 240, 21, 37, 225,
        144, 170, 22, 240, 57, 124, 229, 82, 73, 83, 57, 239, 207, 47, 157, 171, 172, 128, 181,
        245, 217, 139, 151, 55, 227, 52, 52, 105, 187, 174, 43, 39, 67, 128, 83, 211, 250, 106,
        194, 35, 252, 121, 93, 213, 78, 57, 51, 31, 33, 131, 175, 148, 134, 35, 180, 174, 132, 208,
        243, 214, 215, 55, 99, 28, 222, 207, 71, 136, 74, 41, 235, 240, 136, 230, 102, 27, 211,
        170, 2, 19, 106, 235, 113, 245, 245, 125, 90, 147, 186, 178, 166, 86, 184, 22, 135, 102,
        234, 135, 121, 111, 93, 105, 175, 52, 174, 95, 245, 243, 94, 3, 107, 78, 226, 218, 58, 110,
        199, 121, 21, 204, 178, 129, 238, 122, 251, 226, 245, 235, 171, 241, 237, 159, 211, 39, 70,
        84, 78, 208, 209, 205, 48, 219, 32, 125, 229, 116, 160, 51, 110, 99, 151, 174, 108, 223,
        227, 187, 131, 18, 116, 208, 188, 59, 84, 222, 215, 154, 44, 30, 63, 142, 83, 220, 205,
        151, 106, 95, 213, 134, 142, 254, 176, 105, 87, 104, 137, 147, 224, 159, 188, 186, 73, 75,
        112, 164, 119, 202, 146, 47, 155, 14, 247, 41, 79, 38, 236, 122, 114, 131, 174, 117, 121,
        26, 218, 42, 44, 47, 142, 83, 127, 153, 86, 47, 181, 23, 149, 181, 245, 249, 165, 116, 149,
        55, 249, 224, 249, 185, 149, 52, 198, 208, 185, 155, 102, 156, 178, 89, 16, 12, 169, 29,
        217, 48, 181, 187, 120, 9, 113, 106, 227, 252, 16, 11, 62, 242, 130, 108, 94, 55, 211, 12,
        1, 165, 33, 82, 53, 125, 165, 233, 214, 24, 62, 23, 84, 77, 230, 196, 223, 246, 177, 91,
        197, 177, 154, 143, 1, 20, 0, 36, 240, 179, 55, 105, 201, 85, 218, 90, 65, 48, 26, 154,
        182, 107, 187, 219, 217, 30, 91, 137, 224, 233, 156, 20, 174, 121, 85, 87, 210, 209, 238,
        102, 215, 239, 59, 132, 129, 144, 146, 28, 61, 229, 128, 171, 10, 192, 78, 91, 39, 242,
        159, 9, 53, 173, 69, 120, 250, 14, 194, 154, 239, 131, 39, 10, 45, 232, 190, 109, 219, 197,
        102, 200, 142, 116, 170, 82, 128, 199, 217, 145, 58, 167, 65, 159, 81, 41, 82, 224, 221, 1,
        42, 201, 74, 45, 28, 125, 240, 110, 232, 119, 232, 46, 107, 180, 58, 237, 97, 3, 72, 182,
        140, 138, 177, 217, 221, 231, 108, 128, 163, 148, 177, 245, 194, 243, 179, 39, 21, 164,
        164, 175, 41, 138, 111, 54, 148, 57, 198, 27, 114, 239, 253, 208, 67, 70, 205, 55, 107, 64,
        168, 181, 116, 115, 59, 162, 241, 66, 26, 58, 120, 218, 52, 115, 6, 43, 153, 206, 37, 203,
        247, 227, 190, 217, 110, 63, 206, 71, 152, 74, 120, 69, 174, 187, 153, 61, 86, 5, 17, 56,
        249, 250, 105, 131, 104, 244, 194, 74, 62, 150, 210, 73, 90, 175, 158, 9, 91, 90, 2, 186,
        80, 202, 208, 253, 253, 59, 132, 114, 128, 148, 81, 75, 40, 55, 25, 202, 174, 86, 154, 54,
        191, 207, 75, 94, 72, 70, 95, 211, 110, 137, 85, 52, 240, 158, 208, 100, 236, 180, 4, 26,
        88, 172, 30, 98, 91, 129, 203, 252, 17, 182, 33, 37, 141, 97, 140, 244, 251, 1, 31, 23, 4,
        3, 117, 213, 239, 238, 247, 19, 160, 170, 239, 154, 57, 61, 29, 184, 169, 228, 195, 16, 71,
        248, 218, 10, 99, 174, 128, 74, 216, 87, 68, 61, 240, 70, 41, 101, 217, 63, 13, 237, 106,
        138, 235, 57, 236, 38, 164, 8, 135, 7, 97, 183, 198, 81, 216, 119, 240, 178, 121, 213, 0,
        15, 51, 101, 174, 250, 14, 185, 209, 25, 206, 241, 161, 37, 170, 128, 199, 250, 176, 244,
        239, 205, 62, 189, 96, 196, 200, 187, 84, 69, 150, 33, 1, 240, 88, 29, 10, 120, 16, 82, 16,
        38, 193, 233, 13, 177, 31, 243, 58, 128, 21, 8, 93, 151, 119, 254, 21, 205, 134, 74, 67,
        187, 223, 190, 144, 153, 142, 165, 133, 138, 33, 121, 243, 186, 185, 207, 222, 196, 36,
        177, 146, 243, 247, 164, 41, 125, 230, 83, 41, 109, 160, 183, 247, 88, 1, 128, 249, 130,
        165, 131, 57, 41, 225, 233, 154, 147, 178, 237, 222, 245, 195, 142, 239, 3, 190, 146, 198,
        233, 242, 160, 126, 248, 136, 92, 9, 97, 80, 7, 119, 74, 89, 1, 125, 50, 53, 65, 244, 63,
        126, 138, 232, 191, 224, 56, 60, 64, 100, 204, 42, 2, 158, 185, 100, 21, 34, 36, 126, 172,
        176, 252, 176, 5, 137, 132, 192, 169, 252, 117, 215, 78, 25, 3, 182, 242, 53, 87, 139, 4,
        226, 11, 92, 14, 158, 151, 223, 190, 80, 232, 99, 96, 221, 90, 149, 58, 242, 30, 253, 83,
        43, 74, 131, 184, 237, 239, 119, 177, 67, 34, 118, 11, 39, 228, 44, 181, 214, 50, 119, 96,
        28, 146, 176, 224, 220, 104, 114, 93, 116, 86, 177, 111, 187, 248, 1, 195, 30, 36, 187, 99,
        81, 74, 132, 226, 116, 3, 204, 239, 87, 211, 126, 136, 23, 89, 85, 232, 3, 85, 97, 224, 81,
        53, 25, 138, 53, 13, 42, 32, 23, 247, 190, 155, 226, 239, 19, 198, 9, 184, 131, 214, 111,
        135, 108, 148, 178,
    ];

    let mut compressor = Deflator::new(Compression::best());

    let mut f = |v: Vec<_>| {
        let mut compressed = Vec::with_capacity(v.len());
        let r = compressor.compress(&v, &mut compressed);
        println!("{:?}", r);

        let len = compressed.len();
        compressed.truncate(len - 4);
        println!("Output capacity: {}", compressed.capacity());
        println!("Compressed to: {:?}", compressed.len());
    };

    f(v1);
    f(v2);
    f(v3);
}

// #[test]
// fn t() {
//     let mut decompressor = Inflator::new();
//
//
//
//     let mut buffer = Vec::with_capacity(v2.len() * 2);
//
//     let r = decompressor.decompress(&v2, &mut buffer);
//
//     println!("String: {:?}", String::from_utf8(buffer.to_vec()));
//
//     println!("{:?}", r);
// }
