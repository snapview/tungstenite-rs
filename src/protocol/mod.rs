//! Generic WebSocket message stream.

pub mod frame;

mod message;

pub use self::message::Message;
pub use self::frame::CloseFrame;

use std::collections::VecDeque;
use std::io::{ErrorKind as IoErrorKind, Read, Write};
use std::mem::replace;

use error::{Error, Result};
use self::message::{IncompleteMessage, IncompleteMessageType};
use self::frame::{Frame, FrameSocket};
use self::frame::coding::{CloseCode, Control as OpCtl, Data as OpData, OpCode};
use util::NonBlockingResult;

/// Indicates a Client or Server role of the websocket
#[derive(Debug, Clone, Copy)]
pub enum Role {
    /// This socket is a server
    Server,
    /// This socket is a client
    Client,
}

/// WebSocket input-output stream.
///
/// This is THE structure you want to create to be able to speak the WebSocket protocol.
/// It may be created by calling `connect`, `accept` or `client` functions.
#[derive(Debug)]
pub struct WebSocket<Stream> {
    /// Server or client?
    role: Role,
    /// The underlying socket.
    socket: FrameSocket<Stream>,
    /// The state of processing, either "active" or "closing".
    state: WebSocketState,
    /// Receive: an incomplete message being processed.
    incomplete: Option<IncompleteMessage>,
    /// Send: a data send queue.
    send_queue: VecDeque<Frame>,
    /// Send: an OOB pong message.
    pong: Option<Frame>,
}

impl<Stream> WebSocket<Stream> {
    /// Convert a raw socket into a WebSocket without performing a handshake.
    pub fn from_raw_socket(stream: Stream, role: Role) -> Self {
        WebSocket::from_frame_socket(FrameSocket::new(stream), role)
    }

    /// Convert a raw socket into a WebSocket without performing a handshake.
    pub fn from_partially_read(stream: Stream, part: Vec<u8>, role: Role) -> Self {
        WebSocket::from_frame_socket(FrameSocket::from_partially_read(stream, part), role)
    }

    /// Returns a shared reference to the inner stream.
    pub fn get_ref(&self) -> &Stream {
        self.socket.get_ref()
    }
    /// Returns a mutable reference to the inner stream.
    pub fn get_mut(&mut self) -> &mut Stream {
        self.socket.get_mut()
    }

    /// Convert a frame socket into a WebSocket.
    fn from_frame_socket(socket: FrameSocket<Stream>, role: Role) -> Self {
        WebSocket {
            role: role,
            socket: socket,
            state: WebSocketState::Active,
            incomplete: None,
            send_queue: VecDeque::new(),
            pong: None,
        }
    }
}

impl<Stream: Read + Write> WebSocket<Stream> {
    /// Read a message from stream, if possible.
    ///
    /// This function sends pong and close responses automatically.
    /// However, it never blocks on write.
    pub fn read_message(&mut self) -> Result<Message> {
        loop {
            // Since we may get ping or close, we need to reply to the messages even during read.
            // Thus we call write_pending() but ignore its blocking.
            self.write_pending().no_block()?;
            // If we get here, either write blocks or we have nothing to write.
            // Thus if read blocks, just let it return WouldBlock.
            let res = self.read_message_frame();
            if let Some(message) = self.translate_close(res)? {
                trace!("Received message {}", message);
                return Ok(message);
            }
        }
    }

    /// Send a message to stream, if possible.
    ///
    /// This function guarantees that the frame is queued regardless of any errors.
    /// There is no need to resend the frame. In order to handle WouldBlock or Incomplete,
    /// call write_pending() afterwards.
    ///
    /// Note that only the last pong frame is stored to be sent, and only the
    /// most recent pong frame is sent if multiple pong frames are queued up.
    pub fn write_message(&mut self, message: Message) -> Result<()> {
        let frame = match message {
            Message::Text(data) => Frame::message(data.into(), OpCode::Data(OpData::Text), true),
            Message::Binary(data) => Frame::message(data, OpCode::Data(OpData::Binary), true),
            Message::Ping(data) => Frame::ping(data),
            Message::Pong(data) => {
                self.pong = Some(Frame::pong(data));
                return self.write_pending();
            }
        };
        self.send_queue.push_back(frame);
        self.write_pending()
    }

    /// Close the connection.
    ///
    /// This function guarantees that the close frame will be queued.
    /// There is no need to call it again, just like write_message().
    pub fn close(&mut self, code: Option<CloseFrame>) -> Result<()> {
        if let WebSocketState::Active = self.state {
            self.state = WebSocketState::ClosedByUs;
            let frame = Frame::close(code);
            self.send_queue.push_back(frame);
        } else {
            // Already closed, nothing to do.
        }
        self.write_pending()
    }

    /// Flush the pending send queue.
    pub fn write_pending(&mut self) -> Result<()> {
        // First, make sure we have no pending frame sending.
        {
            let res = self.socket.write_pending();
            self.translate_close(res)?;
        }

        // Upon receipt of a Ping frame, an endpoint MUST send a Pong frame in
        // response, unless it already received a Close frame. It SHOULD
        // respond with Pong frame as soon as is practical. (RFC 6455)
        if let Some(pong) = replace(&mut self.pong, None) {
            self.send_one_frame(pong)?;
        }
        // If we have any unsent frames, send them.
        while let Some(data) = self.send_queue.pop_front() {
            self.send_one_frame(data)?;
        }

        // If we're closing and there is nothing to send anymore, we should close the connection.
        if self.send_queue.is_empty() {
            if let WebSocketState::ClosedByPeer(ref mut frame) = self.state {
                // The underlying TCP connection, in most normal cases, SHOULD be closed
                // first by the server, so that it holds the TIME_WAIT state and not the
                // client (as this would prevent it from re-opening the connection for 2
                // maximum segment lifetimes (2MSL), while there is no corresponding
                // server impact as a TIME_WAIT connection is immediately reopened upon
                // a new SYN with a higher seq number). (RFC 6455)
                match self.role {
                    Role::Client => Ok(()),
                    Role::Server => Err(Error::ConnectionClosed(replace(frame, None))),
                }
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    /// Try to decode one message frame. May return None.
    fn read_message_frame(&mut self) -> Result<Option<Message>> {
        if let Some(mut frame) = self.socket.read_frame()? {
            // MUST be 0 unless an extension is negotiated that defines meanings
            // for non-zero values.  If a nonzero value is received and none of
            // the negotiated extensions defines the meaning of such a nonzero
            // value, the receiving endpoint MUST _Fail the WebSocket
            // Connection_.
            if frame.has_rsv1() || frame.has_rsv2() || frame.has_rsv3() {
                return Err(Error::Protocol("Reserved bits are non-zero".into()));
            }

            match self.role {
                Role::Server => {
                    if frame.is_masked() {
                        // A server MUST remove masking for data frames received from a client
                        // as described in Section 5.3. (RFC 6455)
                        frame.remove_mask()
                    } else {
                        // The server MUST close the connection upon receiving a
                        // frame that is not masked. (RFC 6455)
                        return Err(Error::Protocol(
                            "Received an unmasked frame from client".into(),
                        ));
                    }
                }
                Role::Client => {
                    if frame.is_masked() {
                        // A client MUST close a connection if it detects a masked frame. (RFC 6455)
                        return Err(Error::Protocol(
                            "Received a masked frame from server".into(),
                        ));
                    }
                }
            }

            match frame.opcode() {
                OpCode::Control(ctl) => {
                    match ctl {
                        // All control frames MUST have a payload length of 125 bytes or less
                        // and MUST NOT be fragmented. (RFC 6455)
                        _ if !frame.is_final() => {
                            Err(Error::Protocol("Fragmented control frame".into()))
                        }
                        _ if frame.payload().len() > 125 => {
                            Err(Error::Protocol("Control frame too big".into()))
                        }
                        OpCtl::Close => self.do_close(frame.into_close()?).map(|_| None),
                        OpCtl::Reserved(i) => Err(Error::Protocol(
                            format!("Unknown control frame type {}", i).into(),
                        )),
                        OpCtl::Ping | OpCtl::Pong if !self.state.is_active() => {
                            // No ping processing while closing.
                            Ok(None)
                        }
                        OpCtl::Ping => {
                            let data = frame.into_data();
                            self.pong = Some(Frame::pong(data.clone()));
                            Ok(Some(Message::Ping(data)))
                        }
                        OpCtl::Pong => Ok(Some(Message::Pong(frame.into_data()))),
                    }
                }

                OpCode::Data(_) if !self.state.is_active() => {
                    // No data processing while closing.
                    Ok(None)
                }

                OpCode::Data(data) => {
                    let fin = frame.is_final();
                    match data {
                        OpData::Continue => {
                            if let Some(ref mut msg) = self.incomplete {
                                // TODO if msg too big
                                msg.extend(frame.into_data())?;
                            } else {
                                return Err(Error::Protocol(
                                    "Continue frame but nothing to continue".into(),
                                ));
                            }
                            if fin {
                                Ok(Some(replace(&mut self.incomplete, None)
                                    .unwrap()
                                    .complete()?))
                            } else {
                                Ok(None)
                            }
                        }
                        c if self.incomplete.is_some() => Err(Error::Protocol(
                            format!("Received {} while waiting for more fragments", c).into(),
                        )),
                        OpData::Text | OpData::Binary => {
                            let msg = {
                                let message_type = match data {
                                    OpData::Text => IncompleteMessageType::Text,
                                    OpData::Binary => IncompleteMessageType::Binary,
                                    _ => panic!("Bug: message is not text nor binary"),
                                };
                                let mut m = IncompleteMessage::new(message_type);
                                m.extend(frame.into_data())?;
                                m
                            };
                            if fin {
                                Ok(Some(msg.complete()?))
                            } else {
                                self.incomplete = Some(msg);
                                Ok(None)
                            }
                        }
                        OpData::Reserved(i) => Err(Error::Protocol(
                            format!("Unknown data frame type {}", i).into(),
                        )),
                    }
                }
            } // match opcode
        } else {
            match replace(&mut self.state, WebSocketState::Terminated) {
                WebSocketState::CloseAcknowledged(close) | WebSocketState::ClosedByPeer(close) => {
                    Err(Error::ConnectionClosed(close))
                }
                _ => Err(Error::Protocol(
                    "Connection reset without closing handshake".into(),
                )),
            }
        }
    }

    /// Received a close frame.
    fn do_close(&mut self, close: Option<CloseFrame>) -> Result<()> {
        debug!("Received close frame: {:?}", close);
        match self.state {
            WebSocketState::Active => {
                let close_code = close.as_ref().map(|f| f.code);
                self.state = WebSocketState::ClosedByPeer(close.map(CloseFrame::into_owned));
                let reply = if let Some(code) = close_code {
                    if code.is_allowed() {
                        Frame::close(Some(CloseFrame {
                            code: CloseCode::Normal,
                            reason: "".into(),
                        }))
                    } else {
                        Frame::close(Some(CloseFrame {
                            code: CloseCode::Protocol,
                            reason: "Protocol violation".into(),
                        }))
                    }
                } else {
                    Frame::close(None)
                };
                debug!("Replying to close with {:?}", reply);
                self.send_queue.push_back(reply);
                Ok(())
            }
            WebSocketState::ClosedByPeer(_) | WebSocketState::CloseAcknowledged(_) => {
                // It is already closed, just ignore.
                Ok(())
            }
            WebSocketState::ClosedByUs => {
                // We received a reply.
                let close = close.map(CloseFrame::into_owned);
                match self.role {
                    Role::Client => {
                        // Client waits for the server to close the connection.
                        self.state = WebSocketState::CloseAcknowledged(close);
                        Ok(())
                    }
                    Role::Server => {
                        // Server closes the connection.
                        Err(Error::ConnectionClosed(close))
                    }
                }
            }
            WebSocketState::Terminated => unreachable!(),
        }
    }

    /// Send a single pending frame.
    fn send_one_frame(&mut self, mut frame: Frame) -> Result<()> {
        match self.role {
            Role::Server => {}
            Role::Client => {
                // 5.  If the data is being sent by the client, the frame(s) MUST be
                // masked as defined in Section 5.3. (RFC 6455)
                frame.set_mask();
            }
        }
        let res = self.socket.write_frame(frame);
        self.translate_close(res)
    }

    /// Translate a "Connection reset by peer" into ConnectionClosed as needed.
    fn translate_close<T>(&mut self, res: Result<T>) -> Result<T> {
        match res {
            Err(Error::Io(err)) => Err({
                if err.kind() == IoErrorKind::ConnectionReset {
                    match self.state {
                        WebSocketState::ClosedByPeer(ref mut frame) => {
                            Error::ConnectionClosed(replace(frame, None))
                        }
                        WebSocketState::CloseAcknowledged(ref mut frame) => {
                            Error::ConnectionClosed(replace(frame, None))
                        }
                        _ => Error::Io(err),
                    }
                } else {
                    Error::Io(err)
                }
            }),
            x => x,
        }
    }
}

/// The current connection state.
#[derive(Debug)]
enum WebSocketState {
    /// The connection is active.
    Active,
    /// We initiated a close handshake.
    ClosedByUs,
    /// The peer initiated a close handshake.
    ClosedByPeer(Option<CloseFrame<'static>>),
    /// The peer replied to our close handshake.
    CloseAcknowledged(Option<CloseFrame<'static>>),
    /// The connection does not exist anymore.
    Terminated,
}

impl WebSocketState {
    /// Tell if we're allowed to process normal messages.
    fn is_active(&self) -> bool {
        match *self {
            WebSocketState::Active => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Message, Role, WebSocket};

    use std::io;
    use std::io::Cursor;

    struct WriteMoc<Stream>(Stream);

    impl<Stream> io::Write for WriteMoc<Stream> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<Stream: io::Read> io::Read for WriteMoc<Stream> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }

    #[test]
    fn receive_messages() {
        let incoming = Cursor::new(vec![
            0x89, 0x02, 0x01, 0x02, 0x8a, 0x01, 0x03, 0x01, 0x07, 0x48, 0x65, 0x6c, 0x6c, 0x6f,
            0x2c, 0x20, 0x80, 0x06, 0x57, 0x6f, 0x72, 0x6c, 0x64, 0x21, 0x82, 0x03, 0x01, 0x02,
            0x03,
        ]);
        let mut socket = WebSocket::from_raw_socket(WriteMoc(incoming), Role::Client);
        assert_eq!(socket.read_message().unwrap(), Message::Ping(vec![1, 2]));
        assert_eq!(socket.read_message().unwrap(), Message::Pong(vec![3]));
        assert_eq!(
            socket.read_message().unwrap(),
            Message::Text("Hello, World!".into())
        );
        assert_eq!(
            socket.read_message().unwrap(),
            Message::Binary(vec![0x01, 0x02, 0x03])
        );
    }

}
