//! Generic WebSocket message stream.

pub mod frame;

mod message;

pub use self::message::Message;
pub use self::frame::CloseFrame;

use std::collections::VecDeque;
use std::io::{Read, Write, ErrorKind as IoErrorKind};
use std::mem::replace;

use error::{Error, Result};
use self::message::{IncompleteMessage, IncompleteMessageType};
use self::frame::{Frame, FrameSocket};
use self::frame::coding::{OpCode, Data as OpData, Control as OpCtl, CloseCode};
use util::NonBlockingResult;

/// Indicates a Client or Server role of the websocket
#[derive(Debug, Clone, Copy)]
pub enum Role {
    /// This socket is a server
    Server,
    /// This socket is a client
    Client,
}

/// The configuration for WebSocket connection.
#[derive(Debug, Clone, Copy)]
pub struct WebSocketConfig {
    /// The size of the send queue. You can use it to turn on/off the backpressure features. `None`
    /// means here that the size of the queue is unlimited. The default value is the unlimited
    /// queue.
    pub max_send_queue: Option<usize>,
    /// The maximum size of a message. `None` means no size limit. The default value is 64 megabytes
    /// which should be reasonably big for all normal use-cases but small enough to prevent
    /// memory eating by a malicious user.
    pub max_message_size: Option<usize>,
    /// The maximum size of a single message frame. `None` means no size limit. The limit is for
    /// frame payload NOT including the frame header. The default value is 16 megabytes which should
    /// be reasonably big for all normal use-cases but small enough to prevent memory eating
    /// by a malicious user.
    pub max_frame_size: Option<usize>,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        WebSocketConfig {
            max_send_queue: None,
            max_message_size: Some(64 << 20),
            max_frame_size: Some(16 << 20),
        }
    }
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
    /// The configuration for the websocket session.
    config: WebSocketConfig,
}

impl<Stream> WebSocket<Stream> {
    /// Convert a raw socket into a WebSocket without performing a handshake.
    ///
    /// Call this function if you're using Tungstenite as a part of a web framework
    /// or together with an existing one. If you need an initial handshake, use
    /// `connect()` or `accept()` functions of the crate to construct a websocket.
    pub fn from_raw_socket(stream: Stream, role: Role, config: Option<WebSocketConfig>) -> Self {
        WebSocket::from_frame_socket(FrameSocket::new(stream), role, config)
    }

    /// Convert a raw socket into a WebSocket without performing a handshake.
    ///
    /// Call this function if you're using Tungstenite as a part of a web framework
    /// or together with an existing one. If you need an initial handshake, use
    /// `connect()` or `accept()` functions of the crate to construct a websocket.
    pub fn from_partially_read(
        stream: Stream,
        part: Vec<u8>,
        role: Role,
        config: Option<WebSocketConfig>,
    ) -> Self {
        WebSocket::from_frame_socket(FrameSocket::from_partially_read(stream, part), role, config)
    }

    /// Returns a shared reference to the inner stream.
    pub fn get_ref(&self) -> &Stream {
        self.socket.get_ref()
    }
    /// Returns a mutable reference to the inner stream.
    pub fn get_mut(&mut self) -> &mut Stream {
        self.socket.get_mut()
    }

    /// Change the configuration.
    pub fn set_config(&mut self, set_func: impl FnOnce(&mut WebSocketConfig)) {
        set_func(&mut self.config)
    }
}

impl<Stream> WebSocket<Stream> {
    /// Convert a frame socket into a WebSocket.
    fn from_frame_socket(
        socket: FrameSocket<Stream>,
        role: Role,
        config: Option<WebSocketConfig>
    ) -> Self {
        WebSocket {
            role,
            socket,
            state: WebSocketState::Active,
            incomplete: None,
            send_queue: VecDeque::new(),
            pong: None,
            config: config.unwrap_or_else(WebSocketConfig::default),
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
            if let Some(msg) = self.try_read_message()? {
                return Ok(msg);
            }
        }
    }

    /// Read a message from a stream (without blocking), if possible.
    ///
    /// As in [`read_message`], this function sends pong and close
    /// responses automatically in a non-blocking fashion. Unlike
    /// [`read_message`], this allows early exit if the underlying
    /// stream is set to nonblocking mode or has a read timeout.
    ///
    /// [`read_message`]: struct.WebSocket.html#read_message
    pub fn try_read_message(&mut self) -> Result<Option<Message>> {
        // Since we may get ping or close, we need to reply to the messages even during read.
        // Thus we call write_pending() but ignore its blocking.
        self.write_pending().no_block()?;

        // Try to get a frame, convert to None if we get a WouldBlock,
        // strip off any unnecessary wrapping. Additionally, wrap up any
        // closing logic.
        let frame = self.read_message_frame();
        let out = self.translate_close(frame)
            .no_block()?
            .and_then(|x| x);

        if log_enabled!(log::Level::Trace) && out.is_some() {
            trace!("Received message {}", out.as_ref().unwrap());
        }

        Ok(out)
    }

    /// Send a message to stream, if possible.
    ///
    /// WebSocket will buffer a configurable number of messages at a time, except to reply to Ping
    /// and Close requests. If the WebSocket's send queue is full, `SendQueueFull` will be returned
    /// along with the passed message. Otherwise, the message is queued and Ok(()) is returned.
    ///
    /// Note that only the last pong frame is stored to be sent, and only the
    /// most recent pong frame is sent if multiple pong frames are queued.
    pub fn write_message(&mut self, message: Message) -> Result<()> {
        if let Some(max_send_queue) = self.config.max_send_queue {
            if self.send_queue.len() >= max_send_queue {
                // Try to make some room for the new message.
                // Do not return here if write would block, ignore WouldBlock silently
                // since we must queue the message anyway.
                self.write_pending().no_block()?;
            }

            if self.send_queue.len() >= max_send_queue {
                return Err(Error::SendQueueFull(message));
            }
        }

        let frame = match message {
            Message::Text(data) => {
                Frame::message(data.into(), OpCode::Data(OpData::Text), true)
            }
            Message::Binary(data) => {
                Frame::message(data, OpCode::Data(OpData::Binary), true)
            }
            Message::Ping(data) => Frame::ping(data),
            Message::Pong(data) => {
                self.pong = Some(Frame::pong(data));
                return self.write_pending()
            }
        };

        self.send_queue.push_back(frame);
        self.write_pending()
    }

    /// Close the connection.
    ///
    /// This function guarantees that the close frame will be queued.
    /// There is no need to call it again.
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
        if let Some(pong) = self.pong.take() {
            self.send_one_frame(pong)?;
        }
        // If we have any unsent frames, send them.
        while let Some(data) = self.send_queue.pop_front() {
            self.send_one_frame(data)?;
        }

        // If we get to this point, the send queue is empty and the underlying socket is still
        // willing to take more data.

        // If we're closing and there is nothing to send anymore, we should close the connection.
        if let WebSocketState::ClosedByPeer(ref mut frame) = self.state {
            // The underlying TCP connection, in most normal cases, SHOULD be closed
            // first by the server, so that it holds the TIME_WAIT state and not the
            // client (as this would prevent it from re-opening the connection for 2
            // maximum segment lifetimes (2MSL), while there is no corresponding
            // server impact as a TIME_WAIT connection is immediately reopened upon
            // a new SYN with a higher seq number). (RFC 6455)
            match self.role {
                Role::Client => Ok(()),
                Role::Server => Err(Error::ConnectionClosed(frame.take())),
            }
        } else {
            Ok(())
        }
    }
}

impl<Stream: Read + Write> WebSocket<Stream> {
    /// Try to decode one message frame. May return None.
    fn read_message_frame(&mut self) -> Result<Option<Message>> {
        if let Some(mut frame) = self.socket.read_frame(self.config.max_frame_size)? {

            // MUST be 0 unless an extension is negotiated that defines meanings
            // for non-zero values.  If a nonzero value is received and none of
            // the negotiated extensions defines the meaning of such a nonzero
            // value, the receiving endpoint MUST _Fail the WebSocket
            // Connection_.
            {
                let hdr = frame.header();
                if hdr.rsv1 || hdr.rsv2 || hdr.rsv3 {
                    return Err(Error::Protocol("Reserved bits are non-zero".into()))
                }
            }

            match self.role {
                Role::Server => {
                    if frame.is_masked() {
                        // A server MUST remove masking for data frames received from a client
                        // as described in Section 5.3. (RFC 6455)
                        frame.apply_mask()
                    } else {
                        // The server MUST close the connection upon receiving a
                        // frame that is not masked. (RFC 6455)
                        return Err(Error::Protocol("Received an unmasked frame from client".into()))
                    }
                }
                Role::Client => {
                    if frame.is_masked() {
                        // A client MUST close a connection if it detects a masked frame. (RFC 6455)
                        return Err(Error::Protocol("Received a masked frame from server".into()))
                    }
                }
            }

            match frame.header().opcode {

                OpCode::Control(ctl) => {
                    match ctl {
                        // All control frames MUST have a payload length of 125 bytes or less
                        // and MUST NOT be fragmented. (RFC 6455)
                        _ if !frame.header().is_final => {
                            Err(Error::Protocol("Fragmented control frame".into()))
                        }
                        _ if frame.payload().len() > 125 => {
                            Err(Error::Protocol("Control frame too big".into()))
                        }
                        OpCtl::Close => {
                            self.do_close(frame.into_close()?).map(|_| None)
                        }
                        OpCtl::Reserved(i) => {
                            Err(Error::Protocol(format!("Unknown control frame type {}", i).into()))
                        }
                        OpCtl::Ping | OpCtl::Pong if !self.state.is_active() => {
                            // No ping processing while closing.
                            Ok(None)
                        }
                        OpCtl::Ping => {
                            let data = frame.into_data();
                            self.pong = Some(Frame::pong(data.clone()));
                            Ok(Some(Message::Ping(data)))
                        }
                        OpCtl::Pong => {
                            Ok(Some(Message::Pong(frame.into_data())))
                        }
                    }
                }

                OpCode::Data(_) if !self.state.is_active() => {
                    // No data processing while closing.
                    Ok(None)
                }

                OpCode::Data(data) => {
                    let fin = frame.header().is_final;
                    match data {
                        OpData::Continue => {
                            if let Some(ref mut msg) = self.incomplete {
                                msg.extend(frame.into_data(), self.config.max_message_size)?;
                            } else {
                                return Err(Error::Protocol("Continue frame but nothing to continue".into()))
                            }
                            if fin {
                                Ok(Some(self.incomplete.take().unwrap().complete()?))
                            } else {
                                Ok(None)
                            }
                        }
                        c if self.incomplete.is_some() => {
                            Err(Error::Protocol(
                                format!("Received {} while waiting for more fragments", c).into()
                            ))
                        }
                        OpData::Text | OpData::Binary => {
                            let msg = {
                                let message_type = match data {
                                    OpData::Text => IncompleteMessageType::Text,
                                    OpData::Binary => IncompleteMessageType::Binary,
                                    _ => panic!("Bug: message is not text nor binary"),
                                };
                                let mut m = IncompleteMessage::new(message_type);
                                m.extend(frame.into_data(), self.config.max_message_size)?;
                                m
                            };
                            if fin {
                                Ok(Some(msg.complete()?))
                            } else {
                                self.incomplete = Some(msg);
                                Ok(None)
                            }
                        }
                        OpData::Reserved(i) => {
                            Err(Error::Protocol(format!("Unknown data frame type {}", i).into()))
                        }
                    }
                }

            } // match opcode

        } else {
            match replace(&mut self.state, WebSocketState::Terminated) {
                WebSocketState::CloseAcknowledged(close) | WebSocketState::ClosedByPeer(close) => {
                    Err(Error::ConnectionClosed(close))
                }
                _ => {
                    Err(Error::Protocol("Connection reset without closing handshake".into()))
                }
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
                            reason: "Protocol violation".into()
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
            Role::Server => {
            }
            Role::Client => {
                // 5.  If the data is being sent by the client, the frame(s) MUST be
                // masked as defined in Section 5.3. (RFC 6455)
                frame.set_random_mask();
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
                        WebSocketState::ClosedByPeer(ref mut frame) =>
                            Error::ConnectionClosed(frame.take()),
                        WebSocketState::CloseAcknowledged(ref mut frame) =>
                            Error::ConnectionClosed(frame.take()),
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
    use super::{WebSocket, Role, Message, WebSocketConfig};

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
            0x89, 0x02, 0x01, 0x02,
            0x8a, 0x01, 0x03,
            0x01, 0x07,
            0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x2c, 0x20,
            0x80, 0x06,
            0x57, 0x6f, 0x72, 0x6c, 0x64, 0x21,
            0x82, 0x03,
            0x01, 0x02, 0x03,
        ]);
        let mut socket = WebSocket::from_raw_socket(WriteMoc(incoming), Role::Client, None);
        assert_eq!(socket.read_message().unwrap(), Message::Ping(vec![1, 2]));
        assert_eq!(socket.read_message().unwrap(), Message::Pong(vec![3]));
        assert_eq!(socket.read_message().unwrap(), Message::Text("Hello, World!".into()));
        assert_eq!(socket.read_message().unwrap(), Message::Binary(vec![0x01, 0x02, 0x03]));
    }


    #[test]
    fn size_limiting_text_fragmented() {
        let incoming = Cursor::new(vec![
            0x01, 0x07,
            0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x2c, 0x20,
            0x80, 0x06,
            0x57, 0x6f, 0x72, 0x6c, 0x64, 0x21,
        ]);
        let limit = WebSocketConfig {
            max_message_size: Some(10),
            .. WebSocketConfig::default()
        };
        let mut socket = WebSocket::from_raw_socket(WriteMoc(incoming), Role::Client, Some(limit));
        assert_eq!(socket.read_message().unwrap_err().to_string(),
            "Space limit exceeded: Message too big: 7 + 6 > 10"
        );
    }

    #[test]
    fn size_limiting_binary() {
        let incoming = Cursor::new(vec![
            0x82, 0x03,
            0x01, 0x02, 0x03,
        ]);
        let limit = WebSocketConfig {
            max_message_size: Some(2),
            .. WebSocketConfig::default()
        };
        let mut socket = WebSocket::from_raw_socket(WriteMoc(incoming), Role::Client, Some(limit));
        assert_eq!(socket.read_message().unwrap_err().to_string(),
            "Space limit exceeded: Message too big: 0 + 3 > 2"
        );
    }
}
