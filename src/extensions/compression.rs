//!

use std::fmt::{Debug, Display, Formatter};

use crate::extensions::deflate::{DeflateConfig, DeflateExtension};
use crate::extensions::WebSocketExtension;
use crate::protocol::frame::Frame;
use http::header::SEC_WEBSOCKET_EXTENSIONS;
use http::{HeaderValue, Request, Response};

#[derive(Copy, Clone, Debug)]
pub enum CompressionConfig {
    Uncompressed,
    Deflate(DeflateConfig),
}

impl CompressionConfig {
    pub fn into_strategy(self) -> CompressionStrategy {
        match self {
            Self::Uncompressed => CompressionStrategy::Uncompressed,
            Self::Deflate(_config) => CompressionStrategy::Deflate(DeflateExtension::new()),
        }
    }

    pub fn uncompressed() -> CompressionConfig {
        CompressionConfig::Uncompressed
    }

    pub fn deflate() -> CompressionConfig {
        CompressionConfig::Deflate(DeflateConfig::default())
    }
}

pub enum CompressionStrategy {
    Uncompressed,
    Deflate(DeflateExtension),
}

#[derive(Debug, Clone)]
pub struct CompressionExtensionError(String);

impl std::error::Error for CompressionExtensionError {}

impl Display for CompressionExtensionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<CompressionExtensionError> for crate::Error {
    fn from(e: CompressionExtensionError) -> Self {
        crate::Error::ExtensionError(Box::new(e))
    }
}

impl WebSocketExtension for CompressionStrategy {
    type Error = CompressionExtensionError;

    fn on_request<T>(&mut self, request: Request<T>) -> Request<T> {
        match self {
            Self::Uncompressed => request,
            Self::Deflate(de) => de.on_request(request),
        }
    }

    fn on_response<T>(&mut self, response: &Response<T>) {
        match self {
            Self::Uncompressed => {}
            Self::Deflate(de) => de.on_response(response),
        }
    }

    fn on_send_frame(&mut self, frame: Frame) -> Result<Frame, Self::Error> {
        match self {
            Self::Uncompressed => Ok(frame),
            Self::Deflate(de) => de
                .on_send_frame(frame)
                .map_err(|e| CompressionExtensionError(e.to_string())),
        }
    }

    fn on_receive_frame(&mut self, frame: Frame) -> Result<Option<Frame>, Self::Error> {
        match self {
            Self::Uncompressed => Ok(Some(frame)),
            Self::Deflate(de) => de
                .on_receive_frame(frame)
                .map_err(|e| CompressionExtensionError(e.to_string())),
        }
    }
}

impl Debug for CompressionStrategy {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uncompressed => f.debug_struct("Uncompressed").finish(),
            Self::Deflate(_) => f.debug_struct("Deflate").finish(),
        }
    }
}

impl CompressionConfig {
    fn as_header_value(&self) -> Option<HeaderValue> {
        match self {
            Self::Uncompressed => None,
            Self::Deflate(_) => Some(HeaderValue::from_static("permessage-deflate")),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CompressionSelectorError(&'static str);

impl std::error::Error for CompressionSelectorError {}

impl Display for CompressionSelectorError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<CompressionSelectorError> for crate::Error {
    fn from(e: CompressionSelectorError) -> Self {
        crate::Error::ExtensionError(Box::new(e))
    }
}

impl WebSocketExtension for CompressionConfig {
    type Error = CompressionSelectorError;

    fn on_request<T>(&mut self, mut request: Request<T>) -> Request<T> {
        if let Some(header_value) = self.as_header_value() {
            request
                .headers_mut()
                .append(SEC_WEBSOCKET_EXTENSIONS, header_value);
        }

        request
    }

    fn on_response<T>(&mut self, response: &Response<T>) {
        let mut iter = response.headers().get_all(SEC_WEBSOCKET_EXTENSIONS).iter();

        let self_header = match self.as_header_value() {
            Some(hv) => hv,
            None => return,
        };

        match iter.next() {
            Some(hv) if hv == self_header => {}
            _ => {
                *self = CompressionConfig::Uncompressed;
            }
        }
    }
}
