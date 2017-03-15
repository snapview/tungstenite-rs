pub mod coding;

mod frame;

pub use self::frame::Frame;

use std::io::{Read, Write};

use input_buffer::{InputBuffer, MIN_READ};
use error::{Error, Result};

/// A reader and writer for WebSocket frames.
pub struct FrameSocket<Stream> {
    stream: Stream,
    in_buffer: InputBuffer,
    out_buffer: Vec<u8>,
}

impl<Stream> FrameSocket<Stream> {
    /// Create a new frame socket.
    pub fn new(stream: Stream) -> Self {
        FrameSocket {
            stream: stream,
            in_buffer: InputBuffer::with_capacity(MIN_READ),
            out_buffer: Vec::new(),
        }
    }
    /// Create a new frame socket from partially read data.
    pub fn from_partially_read(stream: Stream, part: Vec<u8>) -> Self {
        FrameSocket {
            stream: stream,
            in_buffer: InputBuffer::from_partially_read(part),
            out_buffer: Vec::new(),
        }
    }
    /// Extract a stream from the socket.
    pub fn into_inner(self) -> (Stream, Vec<u8>) {
        (self.stream, self.in_buffer.into_vec())
    }
    /// Returns a shared reference to the inner stream.
    pub fn get_ref(&self) -> &Stream {
        &self.stream
    }
    /// Returns a mutable reference to the inner stream.
    pub fn get_mut(&mut self) -> &mut Stream {
        &mut self.stream
    }
}

impl<Stream> FrameSocket<Stream>
    where Stream: Read
{
    /// Read a frame from stream.
    pub fn read_frame(&mut self) -> Result<Option<Frame>> {
        loop {
            if let Some(frame) = Frame::parse(&mut self.in_buffer.out_mut())? {
                trace!("received frame {}", frame);
                return Ok(Some(frame));
            }
            // No full frames in buffer.
            self.in_buffer.reserve(MIN_READ, usize::max_value())
                .map_err(|_| Error::Capacity("Incoming TCP buffer is full".into()))?;
            let size = self.in_buffer.read_from(&mut self.stream)?;
            if size == 0 {
                trace!("no frame received");
                return Ok(None)
            }
        }
    }

}

impl<Stream> FrameSocket<Stream>
    where Stream: Write
{
    /// Write a frame to stream.
    ///
    /// This function guarantees that the frame is queued regardless of any errors.
    /// There is no need to resend the frame. In order to handle WouldBlock or Incomplete,
    /// call write_pending() afterwards.
    pub fn write_frame(&mut self, frame: Frame) -> Result<()> {
        trace!("writing frame {}", frame);
        self.out_buffer.reserve(frame.len());
        frame.format(&mut self.out_buffer).expect("Bug: can't write to vector");
        self.write_pending()
    }
    /// Complete pending write, if any.
    pub fn write_pending(&mut self) -> Result<()> {
        while !self.out_buffer.is_empty() {
            let len = self.stream.write(&self.out_buffer)?;
            self.out_buffer.drain(0..len);
        }
        Ok(())
    }
}


#[cfg(test)]
mod tests {

    use super::{Frame, FrameSocket};

    use std::io::Cursor;

    #[test]
    fn read_frames() {
        let raw = Cursor::new(vec![
            0x82, 0x07, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
            0x82, 0x03, 0x03, 0x02, 0x01,
            0x99,
        ]);
        let mut sock = FrameSocket::new(raw);

        assert_eq!(sock.read_frame().unwrap().unwrap().into_data(),
            vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07]);
        assert_eq!(sock.read_frame().unwrap().unwrap().into_data(),
            vec![0x03, 0x02, 0x01]);
        assert!(sock.read_frame().unwrap().is_none());

        let (_, rest) = sock.into_inner();
        assert_eq!(rest, vec![0x99]);
    }

    #[test]
    fn from_partially_read() {
        let raw = Cursor::new(vec![
            0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
        ]);
        let mut sock = FrameSocket::from_partially_read(raw, vec![0x82, 0x07, 0x01]);
        assert_eq!(sock.read_frame().unwrap().unwrap().into_data(),
            vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07]);
    }

    #[test]
    fn write_frames() {
        let mut sock = FrameSocket::new(Vec::new());

        let frame = Frame::ping(vec![0x04, 0x05]);
        sock.write_frame(frame).unwrap();

        let frame = Frame::pong(vec![0x01]);
        sock.write_frame(frame).unwrap();

        let (buf, _) = sock.into_inner();
        assert_eq!(buf, vec![
            0x89, 0x02, 0x04, 0x05,
            0x8a, 0x01, 0x01
        ]);
    }

}
