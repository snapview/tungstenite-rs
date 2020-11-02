//! WebSocket extensions

pub mod compression;

use crate::protocol::frame::Frame;
use crate::Message;

/// A trait for defining WebSocket extensions for both WebSocket clients and servers. Extensions
/// may be stacked by nesting them inside one another.
pub trait WebSocketExtension {
    /// Called when a frame is about to be sent.
    fn on_send_frame(&mut self, frame: Frame) -> Result<Frame, crate::Error> {
        Ok(frame)
    }

    /// Called when a frame has been received and unmasked. The frame provided frame will be of the
    /// type `OpCode::Data`.
    fn on_receive_frame(&mut self, frame: Frame) -> Result<Option<Message>, crate::Error>;
}
