//! WebSocket extensions

use http::{Request, Response};

use crate::protocol::frame::Frame;
use crate::Message;

#[cfg(feature = "deflate")]
pub mod deflate;
pub mod uncompressed;

pub trait WebSocketExtension: Default + Clone {
    type Error: Into<crate::Error>;

    fn enabled(&self) -> bool {
        false
    }

    fn rsv1(&self) -> bool {
        false
    }

    fn on_request<T>(&mut self, request: Request<T>) -> Request<T> {
        request
    }

    fn on_response<T>(&mut self, _response: &Response<T>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn on_send_frame(&mut self, frame: Frame) -> Result<Frame, Self::Error> {
        Ok(frame)
    }

    fn on_receive_frame(&mut self, frame: Frame) -> Result<Option<Message>, Self::Error>;
}
