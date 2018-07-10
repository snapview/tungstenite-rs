//! Utilities to work with raw WebSocket frames.

pub mod coding;

mod frame;
mod mask;

pub use self::frame::{Frame, FrameHeader};
pub use self::frame::CloseFrame;

use std::io::{Read, Write};

use input_buffer::{InputBuffer, MIN_READ};
use error::{Error, Result};

/// A reader and writer for WebSocket frames.
#[derive(Debug)]
pub struct FrameSocket<Stream> {
    /// The underlying network stream.
    stream: Stream,
    /// Buffer to read data from the stream.
    in_buffer: InputBuffer,
    /// Buffer to send packets to the network.
    out_buffer: Vec<u8>,
    /// Header and remaining size of the incoming packet being processed.
    header: Option<(FrameHeader, u64)>,
}

impl<Stream> FrameSocket<Stream> {
    /// Create a new frame socket.
    pub fn new(stream: Stream) -> Self {
        FrameSocket {
            stream: stream,
            in_buffer: InputBuffer::with_capacity(MIN_READ),
            out_buffer: Vec::new(),
            header: None,
        }
    }

    /// Create a new frame socket from partially read data.
    pub fn from_partially_read(stream: Stream, part: Vec<u8>) -> Self {
        FrameSocket {
            stream: stream,
            in_buffer: InputBuffer::from_partially_read(part),
            out_buffer: Vec::new(),
            header: None,
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
        let payload = loop {
            {
                let cursor = self.in_buffer.as_cursor_mut();

                if self.header.is_none() {
                    self.header = FrameHeader::parse(cursor)?;
                }

                if let Some((_, ref length)) = self.header {
                    let length = *length;

                    // Make sure `length` is not too big (fits into `usize`).
                    if length > usize::max_value() as u64 {
                        return Err(Error::Capacity(
                            format!("Message length too big: {}", length).into()
                        ))
                    }

                    let input_size = cursor.get_ref().len() as u64 - cursor.position();
                    if length <= input_size {
                        // No truncation here since `length` is checked above
                        let mut payload = Vec::with_capacity(length as usize);
                        if length > 0 {
                            cursor.take(length).read_to_end(&mut payload)?;
                        }
                        break payload
                    }
                }
            }

            // Not enough data in buffer.
            let size = self.in_buffer.prepare_reserve(MIN_READ)
                .with_limit(usize::max_value())
                .map_err(|_| Error::Capacity("Incoming TCP buffer is full".into()))?
                .read_from(&mut self.stream)?;
            if size == 0 {
                trace!("no frame received");
                return Ok(None)
            }
        };

        let (header, length) = self.header.take().expect("Bug: no frame header");
        debug_assert_eq!(payload.len() as u64, length);
        let frame = Frame::from_payload(header, payload);
        trace!("received frame {}", frame);
        Ok(Some(frame))
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
        self.stream.flush()?;
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

    #[test]
    fn parse_overflow() {
        let raw = Cursor::new(vec![
            0x83, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0x00, 0x00, 0x00, 0x00,
        ]);
        let mut sock = FrameSocket::new(raw);
        let _ = sock.read_frame(); // should not crash
    }
}
