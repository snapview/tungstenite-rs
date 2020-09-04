//! WebSocket extensions

use http::{Request, Response};

use crate::protocol::frame::Frame;

pub mod compression;
pub mod deflate;

pub trait WebSocketExtension {
    type Error: Into<crate::Error>;

    fn on_request<T>(&mut self, request: Request<T>) -> Request<T> {
        request
    }

    fn on_response<T>(&mut self, _response: &Response<T>) {}

    fn on_send_frame(&mut self, frame: Frame) -> Result<Frame, Self::Error> {
        Ok(frame)
    }

    fn on_receive_frame(&mut self, frame: Frame) -> Result<Option<Frame>, Self::Error> {
        Ok(Some(frame))
    }
}
