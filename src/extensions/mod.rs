//! WebSocket extensions

use http::{Request, Response};

use crate::protocol::frame::Frame;
use crate::Message;

/// A permessage-deflate WebSocket extension (RFC 7692).
#[cfg(feature = "deflate")]
pub mod deflate;
/// An uncompressed message handler for a WebSocket.
pub mod uncompressed;

/// A trait for defining WebSocket extensions for both WebSocket clients and servers. Extensions
/// may be stacked by nesting them inside one another.
pub trait WebSocketExtension {
    /// An error type that the extension produces.
    type Error: Into<crate::Error>;

    /// Constructs a new WebSocket extension that will permit messages of the provided size.
    fn new(max_message_size: Option<usize>) -> Self;

    /// Returns whether or not the extension is enabled.
    fn enabled(&self) -> bool {
        false
    }

    /// For WebSocket clients, this will be called when a `Request` is being constructed.
    fn on_make_request<T>(&mut self, request: Request<T>) -> Request<T> {
        request
    }

    /// For WebSocket server, this will be called when a `Request` has been received.
    fn on_receive_request<T>(
        &mut self,
        _request: &Request<T>,
        _response: &mut Response<T>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    /// For WebSocket clients, this will be called when a response from the server has been
    /// received. If an error is produced, then subsequent calls to `rsv1()` should return `false`.
    fn on_response<T>(&mut self, _response: &Response<T>) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Called when a frame is about to be sent.
    fn on_send_frame(&mut self, frame: Frame) -> Result<Frame, Self::Error> {
        Ok(frame)
    }

    /// Called when a frame has been received and unmasked. The frame provided frame will be of the
    /// type `OpCode::Data`.
    fn on_receive_frame(&mut self, frame: Frame) -> Result<Option<Message>, Self::Error>;
}
