//! WebSocket extensions

pub mod compression;

use crate::protocol::frame::coding::Data;
use crate::protocol::frame::{ExtensionHeaders, Frame};
use crate::Message;

/// A trait for defining WebSocket extensions for both WebSocket clients and servers. Extensions
/// may be stacked by nesting them inside one another.
pub trait WebSocketExtension {
    /// Called when a frame is about to be sent.
    fn on_send_frame(&mut self, frame: Frame) -> Result<Frame, crate::Error> {
        Ok(frame)
    }

    /// Called when a WebSocket frame has been received.
    fn on_receive_frame(
        &mut self,
        data_opcode: Data,
        is_final: bool,
        header: ExtensionHeaders,
        payload: Vec<u8>,
    ) -> Result<Option<Message>, crate::Error>;
}
