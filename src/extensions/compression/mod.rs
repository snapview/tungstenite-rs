//! WebSocket compression

#[cfg(feature = "deflate")]
use crate::extensions::compression::deflate::{DeflateConfig, DeflateExt};
use crate::extensions::compression::uncompressed::UncompressedExt;
use crate::extensions::WebSocketExtension;
use crate::protocol::frame::coding::Data;
use crate::protocol::frame::{ExtensionHeaders, Frame};
use crate::protocol::WebSocketConfig;
use crate::Message;
use http::{Request, Response};
use std::borrow::Cow;
use std::error::Error;
use std::fmt::{Display, Formatter};

/// A permessage-deflate WebSocket extension (RFC 7692).
#[cfg(feature = "deflate")]
pub mod deflate;
/// An uncompressed message handler for a WebSocket.
pub mod uncompressed;

///
#[derive(Copy, Clone, Debug)]
pub enum WsCompression {
    ///
    None(Option<usize>),
    ///
    #[cfg(feature = "deflate")]
    Deflate(DeflateConfig),
}

/// A WebSocket extension that is either `DeflateExt` or `UncompressedExt`.
#[derive(Debug)]
pub enum CompressionSwitcher {
    ///
    #[cfg(feature = "deflate")]
    Compressed(DeflateExt),
    ///
    Uncompressed(UncompressedExt),
}

impl CompressionSwitcher {
    ///
    pub fn from_config(config: WsCompression) -> CompressionSwitcher {
        match config {
            WsCompression::None(size) => {
                CompressionSwitcher::Uncompressed(UncompressedExt::new(size))
            }
            #[cfg(feature = "deflate")]
            WsCompression::Deflate(config) => {
                CompressionSwitcher::Compressed(DeflateExt::new(config))
            }
        }
    }
}

impl Default for CompressionSwitcher {
    fn default() -> Self {
        CompressionSwitcher::Uncompressed(UncompressedExt::default())
    }
}

#[derive(Debug)]
///
pub struct CompressionError(String);

impl Error for CompressionError {}

impl From<CompressionError> for crate::Error {
    fn from(e: CompressionError) -> Self {
        crate::Error::ExtensionError(Cow::from(e.to_string()))
    }
}

impl Display for CompressionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompressionError")
            .field("error", &self.0)
            .finish()
    }
}

impl WebSocketExtension for CompressionSwitcher {
    fn on_send_frame(&mut self, frame: Frame) -> Result<Frame, crate::Error> {
        match self {
            CompressionSwitcher::Uncompressed(ext) => ext.on_send_frame(frame),
            #[cfg(feature = "deflate")]
            CompressionSwitcher::Compressed(ext) => ext.on_send_frame(frame),
        }
    }

    fn on_receive_frame(
        &mut self,
        data_opcode: Data,
        is_final: bool,
        header: ExtensionHeaders,
        payload: Vec<u8>,
    ) -> Result<Option<Message>, crate::Error> {
        match self {
            CompressionSwitcher::Uncompressed(ext) => {
                ext.on_receive_frame(data_opcode, is_final, header, payload)
            }
            #[cfg(feature = "deflate")]
            CompressionSwitcher::Compressed(ext) => {
                ext.on_receive_frame(data_opcode, is_final, header, payload)
            }
        }
    }
}

///
pub fn build_compression_headers<T>(
    request: Request<T>,
    config: &mut Option<WebSocketConfig>,
) -> Request<T> {
    match config {
        Some(ref mut config) => match &config.compression {
            WsCompression::None(_) => request,
            #[cfg(feature = "deflate")]
            WsCompression::Deflate(config) => deflate::on_make_request(request, config),
        },
        None => request,
    }
}

///
pub fn verify_compression_resp_headers<T>(
    _response: &Response<T>,
    config: &mut Option<WebSocketConfig>,
) -> Result<(), CompressionError> {
    match config {
        Some(ref mut config) => match &mut config.compression {
            WsCompression::None(_) => Ok(()),
            #[cfg(feature = "deflate")]
            WsCompression::Deflate(ref mut deflate_config) => {
                let result = deflate::on_response(_response, deflate_config)
                    .map_err(|e| CompressionError(e.to_string()));

                match result {
                    Ok(true) => Ok(()),
                    Ok(false) => {
                        config.compression = WsCompression::None(deflate_config.max_message_size());
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
        },
        None => Ok(()),
    }
}

///
pub fn verify_compression_req_headers<T>(
    _request: &Request<T>,
    _response: &mut Response<T>,
    config: &mut Option<WebSocketConfig>,
) -> Result<(), CompressionError> {
    match config {
        Some(ref mut config) => match &mut config.compression {
            WsCompression::None(_) => Ok(()),
            #[cfg(feature = "deflate")]
            WsCompression::Deflate(ref mut deflate_config) => {
                let result = deflate::on_receive_request(_request, _response, deflate_config)
                    .map_err(|e| CompressionError(e.to_string()));

                match result {
                    Ok(true) => Ok(()),
                    Ok(false) => {
                        config.compression = WsCompression::None(deflate_config.max_message_size());
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
        },
        None => Ok(()),
    }
}
