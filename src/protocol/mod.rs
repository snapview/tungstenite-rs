//! Generic WebSocket message stream.

pub mod frame;

#[cfg(feature = "deflate")]
pub(crate) mod deflate;

mod message;

pub use self::{frame::CloseFrame, message::Message};

use self::{
    frame::{
        coding::{CloseCode, Control as OpCtl, Data as OpData, OpCode},
        Frame, FrameCodec,
    },
    message::{IncompleteMessage, MessageType},
};
use crate::{
    error::{CapacityError, Error, ProtocolError, Result},
    protocol::frame::Utf8Bytes,
};
use log::*;
use std::{
    io::{self, Read, Write},
    mem::replace,
};

/// Indicates a Client or Server role of the websocket
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// This socket is a server
    Server,
    /// This socket is a client
    Client,
}

/// The configuration for WebSocket connection.
///
/// # Example
/// ```
/// # use tungstenite::protocol::WebSocketConfig;;
/// let conf = WebSocketConfig::default()
///     .read_buffer_size(256 * 1024)
///     .write_buffer_size(256 * 1024);
/// ```
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct WebSocketConfig {
    /// Read buffer capacity. This buffer is eagerly allocated and used for receiving
    /// messages.
    ///
    /// For high read load scenarios a larger buffer, e.g. 128 KiB, improves performance.
    ///
    /// For scenarios where you expect a lot of connections and don't need high read load
    /// performance a smaller buffer, e.g. 4 KiB, would be appropriate to lower total
    /// memory usage.
    ///
    /// The default value is 128 KiB.
    pub read_buffer_size: usize,
    /// The target minimum size of the write buffer to reach before writing the data
    /// to the underlying stream.
    /// The default value is 128 KiB.
    ///
    /// If set to `0` each message will be eagerly written to the underlying stream.
    /// It is often more optimal to allow them to buffer a little, hence the default value.
    ///
    /// Note: [`flush`](WebSocket::flush) will always fully write the buffer regardless.
    pub write_buffer_size: usize,
    /// The max size of the write buffer in bytes. Setting this can provide backpressure
    /// in the case the write buffer is filling up due to write errors.
    /// The default value is unlimited.
    ///
    /// Note: The write buffer only builds up past [`write_buffer_size`](Self::write_buffer_size)
    /// when writes to the underlying stream are failing. So the **write buffer can not
    /// fill up if you are not observing write errors even if not flushing**.
    ///
    /// Note: Should always be at least [`write_buffer_size + 1 message`](Self::write_buffer_size)
    /// and probably a little more depending on error handling strategy.
    /// With deflate enabled, “1 message” means its uncompressed full wire size;
    /// a larger message is unsendable regardless of how much the buffer drains.
    /// A message that compresses larger than its plain form is still sent compressed while
    /// it fits; only when the compressed wire form no longer fits is it sent uncompressed
    /// instead, with outgoing takeover history reset before later compressed output.
    pub max_write_buffer_size: usize,
    /// The maximum size of an incoming message. `None` means no size limit. The default value is 64 MiB
    /// which should be reasonably big for all normal use-cases but small enough to prevent
    /// memory eating by a malicious user.
    pub max_message_size: Option<usize>,
    /// The maximum size of a single incoming message frame. `None` means no size limit. The limit is for
    /// frame payload NOT including the frame header. The default value is 16 MiB which should
    /// be reasonably big for all normal use-cases but small enough to prevent memory eating
    /// by a malicious user.
    pub max_frame_size: Option<usize>,
    /// When set to `true`, the server will accept and handle unmasked frames
    /// from the client. According to the RFC 6455, the server must close the
    /// connection to the client in such cases, however it seems like there are
    /// some popular libraries that are sending unmasked frames, ignoring the RFC.
    /// By default this option is set to `false`, i.e. according to RFC 6455.
    pub accept_unmasked_frames: bool,
    #[cfg(feature = "deflate")]
    pub(crate) deflate: Option<deflate::Settings>,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            read_buffer_size: 128 * 1024,
            write_buffer_size: 128 * 1024,
            max_write_buffer_size: usize::MAX,
            max_message_size: Some(64 << 20),
            max_frame_size: Some(16 << 20),
            accept_unmasked_frames: false,
            #[cfg(feature = "deflate")]
            deflate: None,
        }
    }
}

impl WebSocketConfig {
    /// Offers `permessage-deflate` on this connection, with RFC 7692 defaults.
    ///
    /// A client offers the extension in its handshake; a server accepts an
    /// offer only through [`accept_deflate_offers`]. Enabling it here is not a
    /// promise that the peer agrees — the negotiated outcome is whatever the
    /// handshake settles on, and a peer that declines leaves the connection
    /// uncompressed.
    ///
    /// The client offer carries `client_max_window_bits` with no value, so a server may
    /// select the window this endpoint compresses with; see [`deflate_max_window_bits`].
    ///
    /// [`deflate_max_window_bits`]: WebSocketConfig::deflate_max_window_bits
    ///
    /// [`accept_deflate_offers`]: WebSocketConfig::accept_deflate_offers
    #[cfg(feature = "deflate")]
    pub fn enable_deflate(mut self) -> Self {
        self.deflate.get_or_insert_default();
        self
    }

    /// Caps the LZ77 sliding window for one direction, in bits.
    ///
    /// `role` names the direction, not this endpoint: [`Role::Server`] sets
    /// `server_max_window_bits`, [`Role::Client`] sets `client_max_window_bits`.
    /// Valid values are 9 to 15; a smaller window trades compression ratio for memory.
    ///
    /// Which encoder a setting names decides what it does:
    ///
    /// - **Your own** — [`Role::Client`] on a client, [`Role::Server`] on a server. Always
    ///   applied when permessage-deflate is negotiated, and always saves memory locally.
    /// - **The peer's, on a client** — [`Role::Server`]. A reduced cap is a handshake
    ///   requirement: a response that does not select that cap or less fails the handshake.
    /// - **The peer's, on a server** — [`Role::Client`]. A reduced cap is a preference, not
    ///   a ceiling. An offer carrying `client_max_window_bits` is bound to the cap, or to
    ///   the offer's own lower value. An offer that omits the parameter is still accepted,
    ///   with a bare response and a 15-bit decoder: RFC 7692 §7.1.2.2 forbids naming the
    ///   parameter in a response the offer did not invite, and §7.2.2 then requires a
    ///   32,768-byte decoder window. Such a client bypasses the cap — the decoder window is
    ///   the full 32 KiB rather than the `2^bits` bytes asked for.
    ///   [`accept_deflate_offers`] prefers an offer the cap can bind when a client sends
    ///   more than one.
    ///
    /// A client's own cap is a ceiling rather than a request: the offer names
    /// `client_max_window_bits` without a value, so a server may select this endpoint's
    /// window, and a selection narrows the cap instead of raising it. A selected 8 fails the
    /// handshake — the compressor has no window below 9, and encoding at 9 would exceed what
    /// the server agreed to decode. RFC 7692 §7.1.2.2 reads a response that omits the
    /// parameter as a 32,768-byte peer decoder, so the cap then stands as configured and the
    /// peer must decode whatever we encode with.
    ///
    /// [`accept_deflate_offers`]: WebSocketConfig::accept_deflate_offers
    ///
    /// # Panics
    /// Panics if `bits` is outside 9..=15.
    #[cfg(feature = "deflate")]
    pub fn deflate_max_window_bits(mut self, role: Role, bits: u8) -> Self {
        self.deflate = Some(self.deflate.unwrap_or_default().max_window_bits(role, bits));
        self
    }

    /// Requests that one direction reset its sliding window between messages.
    ///
    /// `role` selects the direction. Disabling context takeover costs most of
    /// the compression ratio — it is the difference between compressing against
    /// the whole conversation and compressing each message alone — and it does
    /// not reduce steady-state memory, because resetting a stream reuses its
    /// arena rather than freeing it. It is a ratio and CPU knob, not a memory
    /// one; for memory use [`deflate_max_window_bits`].
    ///
    /// For a client, setting [`Role::Server`] to `true` is a hard requirement:
    /// a response omitting `server_no_context_takeover` fails the handshake;
    /// preference-plus-fallback is not supported. Per RFC 7692 §7.1.1.2, a
    /// server may ignore this on the client's behalf.
    ///
    /// [`deflate_max_window_bits`]: WebSocketConfig::deflate_max_window_bits
    #[cfg(feature = "deflate")]
    pub fn deflate_no_context_takeover(mut self, role: Role, on: bool) -> Self {
        self.deflate = Some(self.deflate.unwrap_or_default().no_context_takeover(role, on));
        self
    }

    /// Sets the DEFLATE compression level, 0 (store) to 9 (maximum).
    ///
    /// Defaults to the backend's default. Levels above the default cost CPU for
    /// a ratio gain that is small on short messages.
    ///
    /// # Panics
    /// Panics if `level` is above 9.
    #[cfg(feature = "deflate")]
    pub fn deflate_compression_level(mut self, level: u32) -> Self {
        self.deflate = Some(self.deflate.unwrap_or_default().compression_level(level));
        self
    }

    /// Answers a client's extension offers for a server-owned handshake.
    ///
    /// Takes every raw `Sec-WebSocket-Extensions` value from the request and
    /// returns a socket-ready config plus the exact response header, if one was
    /// agreed. On decline, the config has compression disabled and the header
    /// is `None`.
    ///
    /// Which offer wins depends on the [`Role::Client`] window preference from
    /// [`deflate_max_window_bits`]. At its default of 15, the first acceptable offer wins.
    /// Under a reduced preference, an offer carrying `client_max_window_bits` outranks an
    /// earlier one that omits it: only the former lets the response hold the client to the
    /// preference, which is what RFC 7692 §7.1.2.2 gives the parameter for — a response
    /// naming it reduces the memory the server reserves for the connection's decompression
    /// context. Wire order decides within each group, and §7.1.3 lets a server pick any
    /// supported offer. This changes which response header you send, never whether an offer
    /// is acceptable.
    ///
    /// If a header is returned, send it and use the config returned with it;
    /// applying only one would put the wire and codec into different states.
    /// Pass that config to [`WebSocket::from_raw_socket`].
    ///
    /// A framework using tungstenite's own handshake needs only
    /// [`enable_deflate`].
    ///
    /// [`enable_deflate`]: WebSocketConfig::enable_deflate
    /// [`deflate_max_window_bits`]: WebSocketConfig::deflate_max_window_bits
    #[cfg(feature = "deflate")]
    pub fn accept_deflate_offers(
        mut self,
        offers: &[http::HeaderValue],
    ) -> (Self, Option<http::HeaderValue>) {
        let accepted = self.deflate.and_then(|settings| settings.accept_offers(offers));
        let response = accepted.map(|(settings, response)| {
            self.deflate = Some(settings);
            response
        });
        if response.is_none() {
            self.deflate = None;
        }
        (self, response)
    }

    /// Set [`Self::read_buffer_size`].
    pub fn read_buffer_size(mut self, read_buffer_size: usize) -> Self {
        self.read_buffer_size = read_buffer_size;
        self
    }

    /// Set [`Self::write_buffer_size`].
    pub fn write_buffer_size(mut self, write_buffer_size: usize) -> Self {
        self.write_buffer_size = write_buffer_size;
        self
    }

    /// Set [`Self::max_write_buffer_size`].
    pub fn max_write_buffer_size(mut self, max_write_buffer_size: usize) -> Self {
        self.max_write_buffer_size = max_write_buffer_size;
        self
    }

    /// Set [`Self::max_message_size`].
    pub fn max_message_size(mut self, max_message_size: Option<usize>) -> Self {
        self.max_message_size = max_message_size;
        self
    }

    /// Set [`Self::max_frame_size`].
    pub fn max_frame_size(mut self, max_frame_size: Option<usize>) -> Self {
        self.max_frame_size = max_frame_size;
        self
    }

    /// Set [`Self::accept_unmasked_frames`].
    pub fn accept_unmasked_frames(mut self, accept_unmasked_frames: bool) -> Self {
        self.accept_unmasked_frames = accept_unmasked_frames;
        self
    }

    /// Panic if values are invalid.
    pub(crate) fn assert_valid(&self) {
        assert!(
            self.max_write_buffer_size > self.write_buffer_size,
            "WebSocketConfig::max_write_buffer_size must be greater than write_buffer_size, \
            see WebSocketConfig docs`"
        );
    }
}

/// WebSocket input-output stream.
///
/// This is THE structure you want to create to be able to speak the WebSocket protocol.
/// It may be created by calling `connect`, `accept` or `client` functions.
///
/// Use [`WebSocket::read`], [`WebSocket::send`] to received and send messages.
#[derive(Debug)]
pub struct WebSocket<Stream> {
    /// The underlying socket.
    socket: Stream,
    /// The context for managing a WebSocket.
    context: WebSocketContext,
}

impl<Stream> WebSocket<Stream> {
    /// Convert a raw socket into a WebSocket without performing a handshake.
    ///
    /// Call this function if you're using Tungstenite as a part of a web framework
    /// or together with an existing one. If you need an initial handshake, use
    /// `connect()` or `accept()` functions of the crate to construct a websocket.
    ///
    /// # Panics
    /// Panics if config is invalid e.g. `max_write_buffer_size <= write_buffer_size`.
    pub fn from_raw_socket(stream: Stream, role: Role, config: Option<WebSocketConfig>) -> Self {
        WebSocket { socket: stream, context: WebSocketContext::new(role, config) }
    }

    /// Convert a raw socket into a WebSocket without performing a handshake.
    ///
    /// Call this function if you're using Tungstenite as a part of a web framework
    /// or together with an existing one. If you need an initial handshake, use
    /// `connect()` or `accept()` functions of the crate to construct a websocket.
    ///
    /// # Panics
    /// Panics if config is invalid e.g. `max_write_buffer_size <= write_buffer_size`.
    pub fn from_partially_read(
        stream: Stream,
        part: Vec<u8>,
        role: Role,
        config: Option<WebSocketConfig>,
    ) -> Self {
        WebSocket {
            socket: stream,
            context: WebSocketContext::from_partially_read(part, role, config),
        }
    }

    /// Consumes the `WebSocket` and returns the underlying stream.
    pub fn into_inner(self) -> Stream {
        self.socket
    }

    /// Returns a shared reference to the inner stream.
    pub fn get_ref(&self) -> &Stream {
        &self.socket
    }
    /// Returns a mutable reference to the inner stream.
    pub fn get_mut(&mut self) -> &mut Stream {
        &mut self.socket
    }

    /// Change the configuration.
    ///
    /// # Panics
    /// Panics if config is invalid e.g. `max_write_buffer_size <= write_buffer_size`.
    ///
    /// With the `deflate` feature, also panics if the callback changes deflate settings
    /// the handshake has already agreed. Compression is negotiated once, and a connection
    /// cannot move to different settings mid-stream.
    ///
    /// What survives a panic differs between the two builds, in one direction only. An
    /// invalid configuration is committed before it is rejected, in both. With `deflate`
    /// on the callback runs against a copy, so a panic raised *inside* the callback, or a
    /// rejected deflate change, leaves the live configuration untouched.
    pub fn set_config(&mut self, set_func: impl FnOnce(&mut WebSocketConfig)) {
        self.context.set_config(set_func);
    }

    /// Read the configuration.
    pub fn get_config(&self) -> &WebSocketConfig {
        self.context.get_config()
    }

    /// Check if it is possible to read messages.
    ///
    /// Reading is impossible after receiving `Message::Close`. It is still possible after
    /// sending close frame since the peer still may send some data before confirming close.
    pub fn can_read(&self) -> bool {
        self.context.can_read()
    }

    /// Check if it is possible to write messages.
    ///
    /// Writing gets impossible immediately after sending or receiving `Message::Close`.
    pub fn can_write(&self) -> bool {
        self.context.can_write()
    }
}

impl<Stream: Read + Write> WebSocket<Stream> {
    /// Read a message from stream, if possible.
    ///
    /// This will also queue responses to ping and close messages. These responses
    /// will be written and flushed on the next call to [`read`](Self::read),
    /// [`write`](Self::write) or [`flush`](Self::flush).
    ///
    /// # Closing the connection
    /// When the remote endpoint decides to close the connection this will return
    /// the close message with an optional close frame.
    ///
    /// You should continue calling [`read`](Self::read), [`write`](Self::write) or
    /// [`flush`](Self::flush) to drive the reply to the close frame until [`Error::ConnectionClosed`]
    /// is returned. Once that happens it is safe to drop the underlying connection.
    pub fn read(&mut self) -> Result<Message> {
        self.context.read(&mut self.socket)
    }

    /// Writes and immediately flushes a message.
    /// Equivalent to calling [`write`](Self::write) then [`flush`](Self::flush).
    pub fn send(&mut self, message: Message) -> Result<()> {
        self.write(message)?;
        self.flush()
    }

    /// Write a message to the provided stream, if possible.
    ///
    /// A subsequent call should be made to [`flush`](Self::flush) to flush writes.
    ///
    /// In the event of stream write failure the message frame will be stored
    /// in the write buffer and will try again on the next call to [`write`](Self::write)
    /// or [`flush`](Self::flush).
    ///
    /// If the write buffer would exceed the configured [`WebSocketConfig::max_write_buffer_size`]
    /// [`Err(WriteBufferFull(msg_frame))`](Error::WriteBufferFull) is returned.
    ///
    /// This call will generally not flush. However, if there are queued automatic messages
    /// they will be written and eagerly flushed.
    ///
    /// For example, upon receiving ping messages tungstenite queues pong replies automatically.
    /// The next call to [`read`](Self::read), [`write`](Self::write) or [`flush`](Self::flush)
    /// will write & flush the pong reply. This means you should not respond to ping frames manually.
    ///
    /// You can however send pong frames manually in order to indicate a unidirectional heartbeat
    /// as described in [RFC 6455](https://tools.ietf.org/html/rfc6455#section-5.5.3). Note that
    /// if [`read`](Self::read) returns a ping, you should [`flush`](Self::flush) before passing
    /// a custom pong to [`write`](Self::write), otherwise the automatic queued response to the
    /// ping will not be sent as it will be replaced by your custom pong message.
    ///
    /// # Errors
    /// - If the WebSocket's write buffer is full, [`Error::WriteBufferFull`] will be returned
    ///   along with the equivalent passed message frame.
    /// - If the connection is closed and should be dropped, this will return [`Error::ConnectionClosed`].
    /// - If you try again after [`Error::ConnectionClosed`] was returned either from here or from
    ///   [`read`](Self::read), [`Error::AlreadyClosed`] will be returned. This indicates a program
    ///   error on your part.
    /// - [`Error::Io`] is returned if the underlying connection returns an error
    ///   (consider these fatal except for WouldBlock).
    /// - [`Error::Capacity`] if your message size is bigger than the configured max message size.
    pub fn write(&mut self, message: Message) -> Result<()> {
        self.context.write(&mut self.socket, message)
    }

    /// Flush writes.
    ///
    /// Ensures all messages previously passed to [`write`](Self::write) and automatic
    /// queued pong responses are written & flushed into the underlying stream.
    pub fn flush(&mut self) -> Result<()> {
        self.context.flush(&mut self.socket)
    }

    /// Close the connection.
    ///
    /// This function guarantees that the close frame will be queued.
    /// There is no need to call it again. Calling this function is
    /// the same as calling `write(Message::Close(..))`.
    ///
    /// After queuing the close frame you should continue calling [`read`](Self::read) or
    /// [`flush`](Self::flush) to drive the close handshake to completion.
    ///
    /// The websocket RFC defines that the underlying connection should be closed
    /// by the server. Tungstenite takes care of this asymmetry for you.
    ///
    /// When the close handshake is finished (we have both sent and received
    /// a close message), [`read`](Self::read) or [`flush`](Self::flush) will return
    /// [Error::ConnectionClosed] if this endpoint is the server.
    ///
    /// If this endpoint is a client, [Error::ConnectionClosed] will only be
    /// returned after the server has closed the underlying connection.
    ///
    /// It is thus safe to drop the underlying connection as soon as [Error::ConnectionClosed]
    /// is returned from [`read`](Self::read) or [`flush`](Self::flush).
    pub fn close(&mut self, code: Option<CloseFrame>) -> Result<()> {
        self.context.close(&mut self.socket, code)
    }

    /// Old name for [`read`](Self::read).
    #[deprecated(note = "Use `read`")]
    pub fn read_message(&mut self) -> Result<Message> {
        self.read()
    }

    /// Old name for [`send`](Self::send).
    #[deprecated(note = "Use `send`")]
    pub fn write_message(&mut self, message: Message) -> Result<()> {
        self.send(message)
    }

    /// Old name for [`flush`](Self::flush).
    #[deprecated(note = "Use `flush`")]
    pub fn write_pending(&mut self) -> Result<()> {
        self.flush()
    }
}

/// A context for managing WebSocket stream.
#[derive(Debug)]
pub struct WebSocketContext {
    /// Server or client?
    role: Role,
    /// encoder/decoder of frame.
    frame: FrameCodec,
    /// The state of processing, either "active" or "closing".
    state: WebSocketState,
    /// Receive: an incomplete message being processed.
    incomplete: Option<IncompleteMessage>,
    /// Send in addition to regular messages E.g. "pong" or "close".
    additional_send: Option<Frame>,
    /// True indicates there is an additional message (like a pong)
    /// that failed to flush previously and we should try again.
    unflushed_additional: bool,
    /// The configuration for the websocket session.
    config: WebSocketConfig,
    #[cfg(feature = "deflate")]
    deflate: Option<deflate::Context>,
    #[cfg(feature = "deflate")]
    compressed_incomplete: bool,
    /// Compression state is no longer usable, by any of the routes that reach it: a failed
    /// inflate, a decompressed-size limit, a frame carrying compressed payload that was
    /// rejected and discarded, or a frame discarded before anything could ask. The last
    /// one is not known to have desynchronised us -- it is no longer provably in step,
    /// which is the same thing for a decoder. Separate from `WebSocketState`
    /// because the states the base terminates with still permit a final flush; this one
    /// must not.
    #[cfg(feature = "deflate")]
    compression_unusable: bool,
}

impl WebSocketContext {
    /// Create a WebSocket context that manages a post-handshake stream.
    ///
    /// # Panics
    /// Panics if config is invalid e.g. `max_write_buffer_size <= write_buffer_size`.
    pub fn new(role: Role, config: Option<WebSocketConfig>) -> Self {
        let conf = config.unwrap_or_default();
        Self::_new(role, FrameCodec::new(conf.read_buffer_size), conf)
    }

    /// Create a WebSocket context that manages a post-handshake stream.
    ///
    /// # Panics
    /// Panics if config is invalid e.g. `max_write_buffer_size <= write_buffer_size`.
    pub fn from_partially_read(part: Vec<u8>, role: Role, config: Option<WebSocketConfig>) -> Self {
        let conf = config.unwrap_or_default();
        Self::_new(role, FrameCodec::from_partially_read(part, conf.read_buffer_size), conf)
    }

    fn _new(role: Role, mut frame: FrameCodec, config: WebSocketConfig) -> Self {
        config.assert_valid();
        frame.set_max_out_buffer_len(config.max_write_buffer_size);
        frame.set_out_buffer_write_len(config.write_buffer_size);
        #[cfg(feature = "deflate")]
        let deflate = config.deflate.map(|settings| deflate::Context::new(role, settings));
        Self {
            role,
            frame,
            state: WebSocketState::Active,
            incomplete: None,
            additional_send: None,
            unflushed_additional: false,
            config,
            #[cfg(feature = "deflate")]
            deflate,
            #[cfg(feature = "deflate")]
            compressed_incomplete: false,
            #[cfg(feature = "deflate")]
            compression_unusable: false,
        }
    }

    /// Change the configuration.
    ///
    /// # Panics
    /// Panics if config is invalid e.g. `max_write_buffer_size <= write_buffer_size`.
    #[cfg(not(feature = "deflate"))]
    pub fn set_config(&mut self, set_func: impl FnOnce(&mut WebSocketConfig)) {
        set_func(&mut self.config);
        self.config.assert_valid();
        self.frame.set_max_out_buffer_len(self.config.max_write_buffer_size);
        self.frame.set_out_buffer_write_len(self.config.write_buffer_size);
    }

    /// Change the configuration without changing the negotiated compression state.
    ///
    /// # Panics
    /// Panics if config is invalid or the callback changes agreed deflate settings.
    #[cfg(feature = "deflate")]
    pub fn set_config(&mut self, set_func: impl FnOnce(&mut WebSocketConfig)) {
        let mut candidate = self.config;
        set_func(&mut candidate);
        assert_eq!(candidate.deflate, self.config.deflate, "agreed deflate settings are immutable");
        self.config = candidate;
        self.config.assert_valid();
        self.frame.set_max_out_buffer_len(self.config.max_write_buffer_size);
        self.frame.set_out_buffer_write_len(self.config.write_buffer_size);
    }

    /// Read the configuration.
    pub fn get_config(&self) -> &WebSocketConfig {
        &self.config
    }

    /// Check if it is possible to read messages.
    ///
    /// Reading is impossible after receiving `Message::Close`. It is still possible after
    /// sending close frame since the peer still may send some data before confirming close.
    pub fn can_read(&self) -> bool {
        self.state.can_read()
    }

    /// Check if it is possible to write messages.
    ///
    /// Writing gets impossible immediately after sending or receiving `Message::Close`.
    pub fn can_write(&self) -> bool {
        self.state.is_active()
    }

    /// Read a message from the provided stream, if possible.
    ///
    /// This function sends pong and close responses automatically.
    /// However, it never blocks on write.
    pub fn read<Stream>(&mut self, stream: &mut Stream) -> Result<Message>
    where
        Stream: Read + Write,
    {
        // Do not read from already closed connections.
        self.state.check_not_terminated()?;

        loop {
            if self.additional_send.is_some() || self.unflushed_additional {
                // Since we may get ping or close, we need to reply to the messages even during read.
                match self.flush(stream) {
                    Ok(_) => {}
                    Err(Error::Io(err)) if err.kind() == io::ErrorKind::WouldBlock => {
                        // If blocked continue reading, but try again later
                        self.unflushed_additional = true;
                    }
                    Err(err) => return Err(err),
                }
            } else if self.role == Role::Server && !self.state.can_read() {
                self.state = WebSocketState::Terminated;
                return Err(Error::ConnectionClosed);
            }

            // If we get here, either write blocks or we have nothing to write.
            // Thus if read blocks, just let it return WouldBlock.
            if let Some(message) = self.read_message_frame(stream)? {
                trace!("Received message {message}");
                return Ok(message);
            }
        }
    }

    /// Write a message to the provided stream.
    ///
    /// A subsequent call should be made to [`flush`](Self::flush) to flush writes.
    ///
    /// In the event of stream write failure the message frame will be stored
    /// in the write buffer and will try again on the next call to [`write`](Self::write)
    /// or [`flush`](Self::flush).
    ///
    /// If the write buffer would exceed the configured [`WebSocketConfig::max_write_buffer_size`]
    /// [`Err(WriteBufferFull(msg_frame))`](Error::WriteBufferFull) is returned.
    pub fn write<Stream>(&mut self, stream: &mut Stream, message: Message) -> Result<()>
    where
        Stream: Read + Write,
    {
        // When terminated, return AlreadyClosed.
        self.state.check_not_terminated()?;

        // Do not write after sending a close frame.
        if !self.state.is_active() {
            return Err(Error::Protocol(ProtocolError::SendAfterClosing));
        }

        let prepare_data = |this: &mut Self, data, opcode| -> Result<Frame> {
            #[cfg(not(feature = "deflate"))]
            let _ = &this;
            let plain = Frame::message(data, OpCode::Data(opcode), true);
            #[cfg(feature = "deflate")]
            if let Some(deflate) = &mut this.deflate {
                // Keep this role-aware: for clients, `wire_size` counts the mask before
                // `buffer_frame` applies it.
                if !this.frame.can_buffer(wire_size(this.role, &plain)) {
                    return Err(Error::WriteBufferFull(Message::Frame(plain).into()));
                }
                let compressed = match deflate.compress(plain.payload()) {
                    Ok(compressed) => compressed,
                    Err(error) => {
                        deflate.reset_encoder();
                        return Err(error);
                    }
                };
                let mut frame = Frame::message(compressed, OpCode::Data(opcode), true);
                frame.header_mut().rsv1 = true;
                if this.frame.can_buffer(wire_size(this.role, &frame)) {
                    return Ok(frame);
                }
                deflate.reset_encoder();
            }
            Ok(plain)
        };

        let frame = match message {
            Message::Text(data) => prepare_data(self, data.into(), OpData::Text)?,
            Message::Binary(data) => prepare_data(self, data, OpData::Binary)?,
            Message::Ping(data) => Frame::ping(data),
            Message::Pong(data) => {
                self.set_additional(Frame::pong(data));
                // Note: user pongs can be user flushed so no need to flush here
                return self._write(stream, None).map(|_| ());
            }
            Message::Close(code) => return self.close(stream, code),
            Message::Frame(frame) => {
                // The encoder's history is private, so a caller cannot keep it in step with
                // a frame it compressed itself. Under context takeover the next ordinary
                // message would then reference a history the peer does not have.
                #[cfg(feature = "deflate")]
                if frame.header().rsv1 && self.deflate.is_some() {
                    return Err(Error::Protocol(ProtocolError::NonZeroReservedBits));
                }
                frame
            }
        };

        let should_flush = self._write(stream, Some(frame))?;
        if should_flush {
            self.flush(stream)?;
        }
        Ok(())
    }

    /// Flush writes.
    ///
    /// Ensures all messages previously passed to [`write`](Self::write) and automatically
    /// queued pong responses are written & flushed into the `stream`.
    #[inline]
    pub fn flush<Stream>(&mut self, stream: &mut Stream) -> Result<()>
    where
        Stream: Read + Write,
    {
        // Queued bytes belong to a connection the codec failure ended, so they are not
        // written and neither is a close frame -- `close` reaches the stream through here.
        // Latched separately from `WebSocketState` because the states the base sets do
        // permit a final flush, and that is not ours to change.
        #[cfg(feature = "deflate")]
        if self.compression_unusable {
            return Err(Error::AlreadyClosed);
        }
        self._write(stream, None)?;
        self.frame.write_out_buffer(stream)?;
        stream.flush()?;
        self.unflushed_additional = false;
        Ok(())
    }

    /// Writes any data in the out_buffer, `additional_send` and given `data`.
    ///
    /// Does **not** flush.
    ///
    /// Returns true if the write contents indicate we should flush immediately.
    fn _write<Stream>(&mut self, stream: &mut Stream, data: Option<Frame>) -> Result<bool>
    where
        Stream: Read + Write,
    {
        if let Some(data) = data {
            self.buffer_frame(stream, data)?;
        }

        // Upon receipt of a Ping frame, an endpoint MUST send a Pong frame in
        // response, unless it already received a Close frame. It SHOULD
        // respond with Pong frame as soon as is practical. (RFC 6455)
        let should_flush = if let Some(msg) = self.additional_send.take() {
            trace!("Sending pong/close");
            match self.buffer_frame(stream, msg) {
                Err(Error::WriteBufferFull(msg)) => {
                    // if an system message would exceed the buffer put it back in
                    // `additional_send` for retry. Otherwise returning this error
                    // may not make sense to the user, e.g. calling `flush`.
                    if let Message::Frame(msg) = *msg {
                        self.set_additional(msg);
                        false
                    } else {
                        unreachable!()
                    }
                }
                Err(err) => return Err(err),
                Ok(_) => true,
            }
        } else {
            self.unflushed_additional
        };

        // If we're closing and there is nothing to send anymore, we should close the connection.
        if self.role == Role::Server && !self.state.can_read() {
            // The underlying TCP connection, in most normal cases, SHOULD be closed
            // first by the server, so that it holds the TIME_WAIT state and not the
            // client (as this would prevent it from re-opening the connection for 2
            // maximum segment lifetimes (2MSL), while there is no corresponding
            // server impact as a TIME_WAIT connection is immediately reopened upon
            // a new SYN with a higher seq number). (RFC 6455)
            self.frame.write_out_buffer(stream)?;
            self.state = WebSocketState::Terminated;
            Err(Error::ConnectionClosed)
        } else {
            Ok(should_flush)
        }
    }

    /// Close the connection.
    ///
    /// This function guarantees that the close frame will be queued.
    /// There is no need to call it again. Calling this function is
    /// the same as calling `send(Message::Close(..))`.
    pub fn close<Stream>(&mut self, stream: &mut Stream, code: Option<CloseFrame>) -> Result<()>
    where
        Stream: Read + Write,
    {
        if let WebSocketState::Active = self.state {
            self.state = WebSocketState::ClosedByUs;
            let frame = Frame::close(code);
            self._write(stream, Some(frame))?;
        }
        self.flush(stream)
    }

    /// Inflate one frame's payload, ending the connection if the decoder fails.
    ///
    /// A failed inflate has already consumed part of the peer's compressed stream, and
    /// DEFLATE offers no way to resynchronise a decoder, so every later frame would
    /// decode to garbage rather than fail. Terminating keeps `read` and `write` off the
    /// stream, and the separate latch keeps `flush` and `close` off it too -- the states
    /// the base terminates with do allow a last flush, and this one must not.
    #[cfg(feature = "deflate")]
    fn decompress(&mut self, payload: &[u8], final_frame: bool) -> Result<bytes::Bytes> {
        let already = self.incomplete.as_ref().map(IncompleteMessage::len).unwrap_or(0);
        let max_size = self.config.max_message_size;
        let deflate = self.deflate.as_mut().expect("a compressed frame requires a codec");
        deflate.decompress(payload, final_frame, already, max_size).inspect_err(|_| {
            self.fail_compression();
        })
    }

    /// Take the connection out of service because compression state is no longer usable.
    ///
    /// The two fields move together and only here: `WebSocketState` stops `read` and
    /// `write`, and the separate flag also stops `flush` and `close`, which the base's own
    /// terminal states deliberately still allow.
    #[cfg(feature = "deflate")]
    fn fail_compression(&mut self) {
        self.compression_unusable = true;
        self.state = WebSocketState::Terminated;
    }

    /// End the connection when a frame whose payload will never reach our decoder carried
    /// compressed bytes. Every rejection that can be followed by another decode asks this
    /// one function, so the answer cannot differ by which of them asked. A frame arriving
    /// after the read side has closed is discarded without asking, because the states that
    /// close it are one-way and no later decode exists to corrupt.
    ///
    /// `FrameCodec` has already split those bytes off the input buffer, and the peer's
    /// compressor produced them, so our decoder can never see them and -- under context
    /// takeover, the default -- can never rebuild the window they belong to. Discarding
    /// them silently leaves a caller free to read on into wrong bytes, or into an error
    /// blaming a later message that was never at fault.
    #[cfg(feature = "deflate")]
    fn fail_compression_on_discarded_frame(&mut self, frame: &Frame) {
        let header = frame.header();
        // RSV1 is an explicit claim on compression state whatever the opcode, so a
        // discarded control frame carrying it counts too. Without RSV1 only a continuation
        // can be carrying compressed bytes: it does when the message it continues was
        // compressed, and when there is no such message it carries bytes nothing left here
        // can classify -- which under this contract is the same answer. An RSV1-clear
        // continuation of an open plain message is the one that stays live. RSV2 and RSV3
        // claim nothing about PMD.
        // "Claims" rather than "carries": the stray-continuation cell below latches because
        // ownership is unclassifiable, not because those bytes are known to be compressed.
        let claims_compression = header.rsv1
            || (matches!(header.opcode, OpCode::Data(OpData::Continue))
                && (self.compressed_incomplete || self.incomplete.is_none()));
        if claims_compression && self.deflate.is_some() {
            self.fail_compression();
        }
    }

    /// Try to decode one message frame. May return None.
    fn read_message_frame(&mut self, stream: &mut impl Read) -> Result<Option<Message>> {
        let read = self
            .frame
            .read_frame(
                stream,
                self.config.max_frame_size,
                matches!(self.role, Role::Server),
                self.config.accept_unmasked_frames,
            )
            .check_connection_reset(self.state);
        // The only `read_frame` error raised after the payload is split off the input
        // buffer, and it throws the header away with it -- so unlike every other rejection
        // there is nothing left to ask whether those bytes were compressed. Continuing
        // cannot be shown safe, and RFC 6455 requires closing this connection anyway; a
        // caller who needs the frame instead sets `accept_unmasked_frames`, and then no
        // rejection happens here at all.
        #[cfg(feature = "deflate")]
        if self.deflate.is_some()
            && matches!(read, Err(Error::Protocol(ProtocolError::UnmaskedFrameFromClient)))
        {
            self.fail_compression();
        }
        let frame = match read? {
            None => {
                // Connection closed by peer
                return match replace(&mut self.state, WebSocketState::Terminated) {
                    WebSocketState::ClosedByPeer | WebSocketState::CloseAcknowledged => {
                        Err(Error::ConnectionClosed)
                    }
                    _ => Err(Error::Protocol(ProtocolError::ResetWithoutClosingHandshake)),
                };
            }
            Some(frame) => frame,
        };
        #[cfg(feature = "deflate")]
        let mut frame = frame;

        if !self.state.can_read() {
            return Err(Error::Protocol(ProtocolError::ReceivedAfterClosing));
        }
        // MUST be 0 unless an extension is negotiated that defines meanings
        // for non-zero values.  If a nonzero value is received and none of
        // the negotiated extensions defines the meaning of such a nonzero
        // value, the receiving endpoint MUST _Fail the WebSocket
        // Connection_.
        let reserved_bits_set = {
            let hdr = frame.header();
            #[cfg(not(feature = "deflate"))]
            let invalid_rsv1 = hdr.rsv1;
            #[cfg(feature = "deflate")]
            let invalid_rsv1 = hdr.rsv1
                && (self.deflate.is_none()
                    || !matches!(hdr.opcode, OpCode::Data(OpData::Text | OpData::Binary)));
            invalid_rsv1 || hdr.rsv2 || hdr.rsv3
        };
        if reserved_bits_set {
            #[cfg(feature = "deflate")]
            self.fail_compression_on_discarded_frame(&frame);
            return Err(Error::Protocol(ProtocolError::NonZeroReservedBits));
        }

        if self.role == Role::Client && frame.is_masked() {
            // A client MUST close a connection if it detects a masked frame. (RFC 6455)
            #[cfg(feature = "deflate")]
            self.fail_compression_on_discarded_frame(&frame);
            return Err(Error::Protocol(ProtocolError::MaskedFrameFromServer));
        }

        // The fragment-sequence check that rejects an illegal opcode transition lives in
        // the assembly match below, so test the same condition here: a frame that does not
        // own the message stream must not advance the inflater or overwrite the saved
        // compressed mode before it is rejected. Both are one-way.
        #[cfg(feature = "deflate")]
        if let OpCode::Data(data) = frame.header().opcode {
            if matches!(data, OpData::Continue) == self.incomplete.is_some() {
                let final_frame = frame.header().is_final;
                let compressed = match data {
                    OpData::Text | OpData::Binary => frame.header().rsv1,
                    OpData::Continue => self.compressed_incomplete,
                    OpData::Reserved(_) => false,
                };
                if compressed {
                    let payload = self.decompress(frame.payload(), final_frame)?;
                    let mut header = frame.header().clone();
                    header.rsv1 = false;
                    frame = Frame::from_payload(header, payload);
                }
                match data {
                    OpData::Text | OpData::Binary if !final_frame => {
                        self.compressed_incomplete = compressed;
                    }
                    OpData::Continue if final_frame => self.compressed_incomplete = false,
                    _ => {}
                }
            } else {
                // Skipping this frame is not recoverable either: its bytes went through the
                // peer's compressor and its window moved, while ours cannot. Same question
                // as a frame discarded before the host saw it, so the same classifier
                // answers it. The error below is unchanged either way.
                self.fail_compression_on_discarded_frame(&frame);
            }
        }

        match frame.header().opcode {
            OpCode::Control(ctl) => {
                match ctl {
                    // All control frames MUST have a payload length of 125 bytes or less
                    // and MUST NOT be fragmented. (RFC 6455)
                    _ if !frame.header().is_final => {
                        Err(Error::Protocol(ProtocolError::FragmentedControlFrame))
                    }
                    _ if frame.payload().len() > 125 => {
                        Err(Error::Protocol(ProtocolError::ControlFrameTooBig))
                    }
                    OpCtl::Close => Ok(self.do_close(frame.into_close()?).map(Message::Close)),
                    OpCtl::Reserved(i) => {
                        Err(Error::Protocol(ProtocolError::UnknownControlFrameType(i)))
                    }
                    OpCtl::Ping => {
                        let data = frame.into_payload();
                        // No ping processing after we sent a close frame.
                        if self.state.is_active() {
                            self.set_additional(Frame::pong(data.clone()));
                        }
                        Ok(Some(Message::Ping(data)))
                    }
                    OpCtl::Pong => Ok(Some(Message::Pong(frame.into_payload()))),
                }
            }

            OpCode::Data(data) => {
                let fin = frame.header().is_final;

                let payload = match (data, self.incomplete.as_mut()) {
                    (OpData::Continue, None) => Err(ProtocolError::UnexpectedContinueFrame),
                    (OpData::Continue, Some(incomplete)) => {
                        incomplete.extend(frame.into_payload(), self.config.max_message_size)?;
                        Ok(None)
                    }
                    (_, Some(_)) => Err(ProtocolError::ExpectedFragment(data)),
                    (OpData::Text, _) => Ok(Some((frame.into_payload(), MessageType::Text))),
                    (OpData::Binary, _) => Ok(Some((frame.into_payload(), MessageType::Binary))),
                    (OpData::Reserved(i), _) => Err(ProtocolError::UnknownDataFrameType(i)),
                }?;

                match (payload, fin) {
                    (None, true) => Ok(Some(self.incomplete.take().unwrap().complete()?)),
                    (None, false) => Ok(None),
                    (Some((payload, t)), true) => {
                        check_max_size(payload.len(), self.config.max_message_size)?;
                        match t {
                            MessageType::Text => Ok(Some(Message::Text(payload.try_into()?))),
                            MessageType::Binary => Ok(Some(Message::Binary(payload))),
                        }
                    }
                    (Some((payload, t)), false) => {
                        let mut incomplete = IncompleteMessage::new(t);
                        incomplete.extend(payload, self.config.max_message_size)?;
                        self.incomplete = Some(incomplete);
                        Ok(None)
                    }
                }
            }
        } // match opcode
    }

    /// Received a close frame. Tells if we need to return a close frame to the user.
    #[allow(clippy::option_option)]
    fn do_close(&mut self, close: Option<CloseFrame>) -> Option<Option<CloseFrame>> {
        debug!("Received close frame: {close:?}");
        match self.state {
            WebSocketState::Active => {
                self.state = WebSocketState::ClosedByPeer;

                let close = close.map(|frame| {
                    if !frame.code.is_allowed() {
                        CloseFrame {
                            code: CloseCode::Protocol,
                            reason: Utf8Bytes::from_static("Protocol violation"),
                        }
                    } else {
                        frame
                    }
                });

                let reply = Frame::close(close.clone());
                debug!("Replying to close with {reply:?}");
                self.set_additional(reply);

                Some(close)
            }
            WebSocketState::ClosedByPeer | WebSocketState::CloseAcknowledged => {
                // It is already closed, just ignore.
                None
            }
            WebSocketState::ClosedByUs => {
                // We received a reply.
                self.state = WebSocketState::CloseAcknowledged;
                Some(close)
            }
            WebSocketState::Terminated => unreachable!(),
        }
    }

    /// Write a single frame into the write-buffer.
    fn buffer_frame<Stream>(&mut self, stream: &mut Stream, mut frame: Frame) -> Result<()>
    where
        Stream: Read + Write,
    {
        match self.role {
            Role::Server => {}
            Role::Client => {
                // 5.  If the data is being sent by the client, the frame(s) MUST be
                // masked as defined in Section 5.3. (RFC 6455)
                frame.set_random_mask();
            }
        }

        trace!("Sending frame: {frame:?}");
        self.frame.buffer_frame(stream, frame).check_connection_reset(self.state)
    }

    /// Replace `additional_send` if it is currently a `Pong` message.
    fn set_additional(&mut self, add: Frame) {
        let empty_or_pong = self
            .additional_send
            .as_ref()
            .is_none_or(|f| f.header().opcode == OpCode::Control(OpCtl::Pong));
        if empty_or_pong {
            self.additional_send.replace(add);
        }
    }
}

/// Wire bytes a prepared frame will occupy once `buffer_frame` has masked it.
///
/// Only ever called on a frame built moments earlier in `write`, so the client mask is
/// always still pending.
#[cfg(feature = "deflate")]
fn wire_size(role: Role, frame: &Frame) -> usize {
    debug_assert!(!frame.is_masked(), "wire_size adds the pending client mask itself");
    frame.len() + usize::from(role == Role::Client) * 4
}

fn check_max_size(size: usize, max_size: Option<usize>) -> crate::Result<()> {
    if let Some(max_size) = max_size {
        if size > max_size {
            return Err(Error::Capacity(CapacityError::MessageTooLong { size, max_size }));
        }
    }
    Ok(())
}

/// The current connection state.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum WebSocketState {
    /// The connection is active.
    Active,
    /// We initiated a close handshake.
    ClosedByUs,
    /// The peer initiated a close handshake.
    ClosedByPeer,
    /// The peer replied to our close handshake.
    CloseAcknowledged,
    /// The connection does not exist anymore.
    Terminated,
}

impl WebSocketState {
    /// Tell if we're allowed to process normal messages.
    fn is_active(self) -> bool {
        matches!(self, WebSocketState::Active)
    }

    /// Tell if we should process incoming data. Note that if we send a close frame
    /// but the remote hasn't confirmed, they might have sent data before they receive our
    /// close frame, so we should still pass those to client code, hence ClosedByUs is valid.
    fn can_read(self) -> bool {
        matches!(self, WebSocketState::Active | WebSocketState::ClosedByUs)
    }

    /// Check if the state is active, return error if not.
    fn check_not_terminated(self) -> Result<()> {
        match self {
            WebSocketState::Terminated => Err(Error::AlreadyClosed),
            _ => Ok(()),
        }
    }
}

/// Translate "Connection reset by peer" into `ConnectionClosed` if appropriate.
trait CheckConnectionReset {
    fn check_connection_reset(self, state: WebSocketState) -> Self;
}

impl<T> CheckConnectionReset for Result<T> {
    fn check_connection_reset(self, state: WebSocketState) -> Self {
        match self {
            Err(Error::Io(io_error)) => Err({
                if !state.can_read() && io_error.kind() == io::ErrorKind::ConnectionReset {
                    Error::ConnectionClosed
                } else {
                    Error::Io(io_error)
                }
            }),
            x => x,
        }
    }
}

/// Read-only duplex: the deflate read rows drive `read()` over fixed frame bytes.
#[cfg(all(test, feature = "deflate"))]
mod incoming {
    use std::io::{self, Cursor};

    pub(super) struct Incoming(pub(super) Cursor<Vec<u8>>);

    impl io::Read for Incoming {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            io::Read::read(&mut self.0, buf)
        }
    }

    impl io::Write for Incoming {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(all(test, feature = "deflate"))]
mod rfc_7692_section_6_1 {
    use super::{incoming::Incoming, *};
    use crate::error::ProtocolError;
    use std::io::Cursor;

    /// One row per rule, and the rules come from two documents: RFC 7692 §6.1 forbids
    /// the compressed bit on control frames and on non-first fragments, while RFC 6455
    /// §5.2 forbids any reserved bit no negotiated extension has defined. All three
    /// arrive at one `invalid_rsv1` decision reporting `NonZeroReservedBits`, so the
    /// rows stay separate while pinning a single variant.
    fn reads_as_reserved_bits_error(frames: &[u8], deflate: bool) {
        let config = if deflate {
            WebSocketConfig::default().enable_deflate()
        } else {
            WebSocketConfig::default()
        };
        let mut socket = WebSocket::from_raw_socket(
            Incoming(Cursor::new(frames.to_vec())),
            Role::Client,
            Some(config),
        );
        assert!(
            matches!(
                socket.read().unwrap_err(),
                Error::Protocol(ProtocolError::NonZeroReservedBits)
            ),
            "RSV1 must be rejected here"
        );
    }

    /// RSV1 with no extension negotiated: nothing defines the bit's meaning.
    #[test]
    fn rsv1_without_a_negotiated_extension_is_rejected() {
        reads_as_reserved_bits_error(&[0x41, 0x03, 0xf2, 0x48, 0xcd], false);
    }

    /// RSV1 on a control frame, negotiated or not. FIN + RSV1 + Ping, empty.
    #[test]
    fn rsv1_on_a_control_frame_is_rejected() {
        reads_as_reserved_bits_error(&[0xc9, 0x00], true);
    }

    /// RSV1 on a continuation: §6.1 allows the bit only on the first fragment.
    /// The two-frame message from RFC 7692 §7.2.3.2 with RSV1 set on the second.
    #[test]
    fn rsv1_on_a_continuation_frame_is_rejected() {
        reads_as_reserved_bits_error(
            &[0x41, 0x03, 0xf2, 0x48, 0xcd, 0xc0, 0x04, 0xc9, 0xc9, 0x07, 0x00],
            true,
        );
    }

    /// The control, without which the three rows above could pass because
    /// everything errors: the same first fragment plus a *clean* continuation is
    /// a legal compressed message and must decode.
    #[test]
    fn control_the_same_message_without_rsv1_on_the_continuation_decodes() {
        let mut socket = WebSocket::from_raw_socket(
            Incoming(Cursor::new(vec![
                0x41, 0x03, 0xf2, 0x48, 0xcd, 0x80, 0x04, 0xc9, 0xc9, 0x07, 0x00,
            ])),
            Role::Client,
            Some(WebSocketConfig::default().enable_deflate()),
        );
        assert_eq!(socket.read().expect("a legal compressed message"), Message::text("Hello"));
    }
}

#[cfg(all(test, feature = "deflate"))]
mod codec_state_ownership {
    use super::{incoming::Incoming, *};
    use crate::error::ProtocolError;
    use std::io::Cursor;

    fn client(frames: &[u8]) -> WebSocket<Incoming> {
        WebSocket::from_raw_socket(
            Incoming(Cursor::new(frames.to_vec())),
            Role::Client,
            Some(WebSocketConfig::default().enable_deflate()),
        )
    }

    /// The compressed "Hello" of RFC 7692 7.2.3.2, split as the RFC splits it.
    const FIRST_FRAGMENT: &[u8] = &[0xf2, 0x48, 0xcd];
    const LAST_FRAGMENT: &[u8] = &[0xc9, 0xc9, 0x07, 0x00];

    /// A frame the host will reject must not reach the codec first. Here a stray
    /// plain Text arrives during an open compressed Binary message: it is illegal,
    /// but at the point the deflate precursor sees it, it looks like the start of
    /// an uncompressed message and clears the saved mode. The real continuation
    /// then decodes as raw deflate bytes and `complete()` accepts them. The stray
    /// itself is reported; what is silent is the corruption, handed to the caller
    /// as `Ok` on the following read.
    #[test]
    fn a_rejected_stray_frame_does_not_clear_the_saved_compressed_mode() {
        let mut frames = vec![0x42, 0x03];
        frames.extend_from_slice(FIRST_FRAGMENT);
        frames.extend_from_slice(&[0x01, 0x01, b'A']);
        frames.extend_from_slice(&[0x80, 0x04]);
        frames.extend_from_slice(LAST_FRAGMENT);

        let mut socket = client(&frames);
        assert!(
            matches!(
                socket.read().unwrap_err(),
                Error::Protocol(ProtocolError::ExpectedFragment(_))
            ),
            "a new data frame during an open message is illegal"
        );
        assert_eq!(
            socket.read().expect("the continuation is still part of a compressed message"),
            Message::binary(b"Hello".to_vec())
        );
    }

    /// Two messages through one peer compressor, the second repeating a long run from the
    /// first so it can only decode against a window that holds it.
    const FIRST: &[u8] = b"permessage-deflate context takeover shares one window";
    const SECOND: &[u8] = b"context takeover shares one window, so this back-references";

    fn peer_pair() -> (Vec<u8>, Vec<u8>) {
        let mut peer = deflate::Context::new(Role::Server, deflate::Settings::default());
        let first = peer.compress(FIRST).expect("the peer compresses its first message");
        let second = peer.compress(SECOND).expect("the peer compresses its second message");
        (first.to_vec(), second.to_vec())
    }

    fn frame_bytes(fin: bool, rsv1: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() < 126, "the fixture keeps every header two bytes");
        let first = (u8::from(fin) << 7) | (u8::from(rsv1) << 6) | opcode;
        let mut out = vec![first, payload.len() as u8];
        out.extend_from_slice(payload);
        out
    }

    /// The ordering rule in the other direction, and the reason skipping is not enough on
    /// its own: this frame's bytes went through the peer's compressor, so declining to
    /// decode them leaves its window holding plaintext ours never will. A later legal
    /// message back-references that plaintext, and the mismatch produces wrong bytes rather
    /// than an error, so the connection ends here instead.
    #[test]
    fn a_skipped_compressed_message_ends_the_connection() {
        let (first, _) = peer_pair();
        let mut frames = frame_bytes(false, false, 0x2, b"open");
        frames.extend(frame_bytes(true, true, 0x2, &first));
        frames.extend(frame_bytes(true, false, 0x0, b"close"));

        let mut socket = client(&frames);
        assert!(matches!(
            socket.read().unwrap_err(),
            Error::Protocol(ProtocolError::ExpectedFragment(_))
        ));
        assert!(
            matches!(socket.read().unwrap_err(), Error::AlreadyClosed),
            "the message that would have completed must not be read against a stale window"
        );
    }

    /// Without which the row above pins a connection ending for no reason. The second
    /// message has to genuinely need the first in the decoder, or skipping one costs
    /// nothing and terminating is unjustified.
    #[test]
    fn control_the_second_message_needs_the_first_in_the_decoder() {
        let (first, second) = peer_pair();

        let mut both = frame_bytes(true, true, 0x2, &first);
        both.extend(frame_bytes(true, true, 0x2, &second));
        let mut socket = client(&both);
        assert_eq!(socket.read().expect("first message"), Message::binary(FIRST.to_vec()));
        assert_eq!(socket.read().expect("second message"), Message::binary(SECOND.to_vec()));

        let mut alone = client(&frame_bytes(true, true, 0x2, &second));
        assert!(
            !matches!(alone.read(), Ok(Message::Binary(ref bytes)) if bytes.as_ref() == SECOND),
            "the second message must not decode correctly against a window without the first"
        );
    }

    fn masked_frame_bytes(fin: bool, rsv1: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        const KEY: [u8; 4] = [0xa1, 0xb2, 0xc3, 0xd4];
        assert!(payload.len() < 126, "the fixture keeps every header two bytes");
        let first = (u8::from(fin) << 7) | (u8::from(rsv1) << 6) | opcode;
        let mut out = vec![first, 0x80 | payload.len() as u8];
        out.extend_from_slice(&KEY);
        out.extend(payload.iter().enumerate().map(|(i, byte)| byte ^ KEY[i % 4]));
        out
    }

    fn server(frames: &[u8]) -> WebSocket<Wire> {
        WebSocket::from_raw_socket(
            Wire::new(frames.to_vec()),
            Role::Server,
            Some(WebSocketConfig::default().enable_deflate()),
        )
    }

    /// `FrameCodec` splits the whole payload off the input buffer before the host gets to
    /// classify it, so a frame rejected here is already gone. When the peer's compressor
    /// produced those bytes, our decoder can never see them, and reading on reaches wrong
    /// bytes or an error blaming a later message. These are the whole-frame rejections;
    /// a header-parse or frame-size error leaves the bytes unread and is not affected.
    #[test]
    fn a_discarded_compressed_frame_ends_the_connection() {
        let (first, second) = peer_pair();

        // Reserved bits, on a compressed message the peer's compressor already consumed.
        let mut frames = frame_bytes(true, true, 0x2, &first);
        frames[0] |= 0x20;
        frames.extend(frame_bytes(true, true, 0x2, &second));
        let mut socket = client(&frames);
        assert!(matches!(
            socket.read().unwrap_err(),
            Error::Protocol(ProtocolError::NonZeroReservedBits)
        ));
        assert!(matches!(socket.read().unwrap_err(), Error::AlreadyClosed));

        // A masked frame from a server, same shape.
        let mut frames = masked_frame_bytes(true, true, 0x2, &first);
        frames.extend(frame_bytes(true, true, 0x2, &second));
        let mut socket = client(&frames);
        assert!(matches!(
            socket.read().unwrap_err(),
            Error::Protocol(ProtocolError::MaskedFrameFromServer)
        ));
        assert!(matches!(socket.read().unwrap_err(), Error::AlreadyClosed));
    }

    /// The server-role twin, where `FrameCodec` rejects before the host sees a frame at
    /// all -- so the decision can only rest on a compressed message being open.
    #[test]
    fn a_discarded_unmasked_continuation_ends_the_connection() {
        let (first, _) = peer_pair();
        let (head, tail) = first.split_at(first.len() / 2);
        let mut frames = masked_frame_bytes(false, true, 0x2, head);
        frames.extend(frame_bytes(false, false, 0x0, tail));
        let mut socket = server(&frames);

        assert!(matches!(
            socket.read().unwrap_err(),
            Error::Protocol(ProtocolError::UnmaskedFrameFromClient)
        ));
        assert!(matches!(socket.read().unwrap_err(), Error::AlreadyClosed));
        assert!(matches!(socket.flush().unwrap_err(), Error::AlreadyClosed));
    }

    /// A complete compressed message, unmasked, with nothing in flight. `FrameCodec`
    /// throws the header away along with the payload, so by the time the host hears about
    /// it there is nothing left to ask whether those bytes were compressed. They were: the
    /// peer's compressor produced them and its window moved. The second message is legal
    /// and back-references the first, which is what a stale decoder would get wrong.
    fn unmasked_whole_message() -> Vec<u8> {
        let (first, second) = peer_pair();
        let mut frames = frame_bytes(true, true, 0x2, &first);
        frames.extend(masked_frame_bytes(true, true, 0x2, &second));
        frames
    }

    /// A server holding queued output, one unmasked rejection into the stream.
    fn server_after_unmasked_rejection() -> WebSocket<Wire> {
        let config = WebSocketConfig::default().enable_deflate().write_buffer_size(64 * 1024);
        let mut socket = WebSocket::from_raw_socket(
            Wire::new(unmasked_whole_message()),
            Role::Server,
            Some(config),
        );
        socket.write(Message::binary(b"queued".to_vec())).expect("the frame queues");
        assert!(socket.get_ref().written.is_empty(), "the fixture must leave the frame queued");
        assert!(matches!(
            socket.read().unwrap_err(),
            Error::Protocol(ProtocolError::UnmaskedFrameFromClient)
        ));
        socket
    }

    #[test]
    fn a_discarded_unmasked_message_stops_later_reads() {
        let mut socket = server_after_unmasked_rejection();
        assert!(matches!(socket.read().unwrap_err(), Error::AlreadyClosed));
        nothing_reached_the_stream(&socket);
    }

    /// Its own socket: asserted after the read row on a shared one, the first assertion
    /// panics on any tree without the fix and this arm is never observed.
    #[test]
    fn a_discarded_unmasked_message_stops_later_flushes() {
        let mut socket = server_after_unmasked_rejection();
        assert!(matches!(socket.flush().unwrap_err(), Error::AlreadyClosed));
        nothing_reached_the_stream(&socket);
    }

    #[test]
    fn a_discarded_unmasked_message_stops_later_closes() {
        let mut socket = server_after_unmasked_rejection();
        assert!(matches!(socket.close(None).unwrap_err(), Error::AlreadyClosed));
        nothing_reached_the_stream(&socket);
    }

    /// The compatibility contract the conservative latch rests on: a caller who needs the
    /// frame sets `accept_unmasked_frames`, and then there is no rejection to latch on.
    #[test]
    fn control_accepting_unmasked_frames_decodes_the_same_message() {
        let (first, _) = peer_pair();
        let config = WebSocketConfig::default().enable_deflate().accept_unmasked_frames(true);
        let mut socket = WebSocket::from_raw_socket(
            Wire::new(frame_bytes(true, true, 0x2, &first)),
            Role::Server,
            Some(config),
        );
        assert_eq!(
            socket.read().expect("the caller asked for this frame"),
            Message::binary(FIRST.to_vec())
        );
    }

    /// A message compressed by an encoder of its own, so it decodes against any window and
    /// tests the connection rather than the history.
    fn standalone_compressed(payload: &[u8]) -> Vec<u8> {
        let mut peer = deflate::Context::new(Role::Server, deflate::Settings::default());
        peer.compress(payload).expect("the peer compresses a standalone message").to_vec()
    }

    /// The classifier's truth table, one row per cell that is not already pinned above.
    /// RSV1 is an explicit claim on compression state whatever the opcode carries it.
    #[test]
    fn a_discarded_frame_claiming_compression_ends_the_connection() {
        let compressed = standalone_compressed(FIRST);

        // A continuation with no message to continue: nothing is left to classify it by,
        // whether it claims compression or not, and at either discarding exit.
        let mut stray_rsv2 = frame_bytes(true, false, 0x0, &compressed);
        stray_rsv2[0] |= 0x20;
        for frames in [
            stray_rsv2,
            frame_bytes(true, true, 0x0, &compressed),
            masked_frame_bytes(true, false, 0x0, &compressed),
        ] {
            let mut socket = client(&frames);
            assert!(socket.read().is_err(), "the stray continuation is rejected");
            assert!(matches!(socket.read().unwrap_err(), Error::AlreadyClosed));
        }

        // A plain continuation of an open *compressed* message: no RSV1 claim of its own,
        // but the message it continues had one.
        let (head, tail) = compressed.split_at(compressed.len() / 2);
        let mut frames = frame_bytes(false, true, 0x2, head);
        let mut discarded = frame_bytes(true, false, 0x0, tail);
        discarded[0] |= 0x20;
        frames.extend(discarded);
        let mut socket = client(&frames);
        assert!(matches!(
            socket.read().unwrap_err(),
            Error::Protocol(ProtocolError::NonZeroReservedBits)
        ));
        assert!(matches!(socket.read().unwrap_err(), Error::AlreadyClosed));

        // RSV1 on a continuation while a *plain* message is open. It is in position, so it
        // never reaches the fragment-sequence branch, and `compressed_incomplete` is false
        // -- only the RSV1 claim itself makes this unusable.
        let mut frames = vec![0x02, 0x03, b'a', b'b', b'c'];
        frames.extend(frame_bytes(true, true, 0x0, &compressed));
        let mut socket = client(&frames);
        assert!(matches!(
            socket.read().unwrap_err(),
            Error::Protocol(ProtocolError::NonZeroReservedBits)
        ));
        assert!(matches!(socket.read().unwrap_err(), Error::AlreadyClosed));

        // RSV1 on a control frame: FIN + RSV1 + Ping, empty.
        let mut socket = client(&[0xc9, 0x00]);
        assert!(matches!(
            socket.read().unwrap_err(),
            Error::Protocol(ProtocolError::NonZeroReservedBits)
        ));
        assert!(matches!(socket.read().unwrap_err(), Error::AlreadyClosed));
    }

    /// The false cell, without which the row above is satisfied by latching on everything:
    /// an RSV1-clear continuation of an open plain message carries no compressed bytes and
    /// nothing about the peer's window changed, so discarding it costs nothing.
    #[test]
    fn control_a_discarded_plain_continuation_leaves_the_connection_open() {
        // Open a plain message, discard one continuation on RSV2, then finish the message
        // and send a compressed one. The discarded bytes are simply lost, as they are at
        // base -- what must survive is the connection.
        let mut frames = vec![
            0x02, 0x03, b'a', b'b', b'c', // plain, non-final
            0x20, 0x02, b'd', b'e', // continuation with RSV2: discarded
            0x80, 0x02, b'f', b'g', // continuation, final
        ];
        frames.extend(frame_bytes(true, true, 0x2, &standalone_compressed(FIRST)));
        let mut socket = client(&frames);

        assert!(matches!(
            socket.read().unwrap_err(),
            Error::Protocol(ProtocolError::NonZeroReservedBits)
        ));
        assert_eq!(
            socket.read().expect("nothing compressed was discarded"),
            Message::binary(b"abcfg".to_vec()),
            "the message completes without the frame that was thrown away"
        );
        assert_eq!(
            socket.read().expect("and the decoder is still usable"),
            Message::binary(FIRST.to_vec()),
            "a later compressed message decodes exactly"
        );
    }

    /// The second false cell: a control frame making no compression claim. Discarding it
    /// says nothing about the peer's window, so the connection carries on.
    #[test]
    fn control_a_discarded_plain_control_frame_leaves_the_connection_open() {
        // FIN | RSV2 | Ping, empty -- rejected for the reserved bit, claiming no RSV1.
        let mut frames = vec![0xa9, 0x00];
        frames.extend(frame_bytes(true, true, 0x2, &standalone_compressed(FIRST)));
        let mut socket = client(&frames);

        assert!(matches!(
            socket.read().unwrap_err(),
            Error::Protocol(ProtocolError::NonZeroReservedBits)
        ));
        assert_eq!(
            socket.read().expect("a plain control frame claims nothing"),
            Message::binary(FIRST.to_vec())
        );
    }

    /// `read` refuses on `WebSocketState` alone, so a row that only reads cannot tell
    /// whether the compression latch moved. `flush` is the observation that can: replacing
    /// the classifier's `fail_compression()` with a bare state assignment passes every
    /// read-only row and fails these two.
    #[test]
    fn a_discarded_compressed_frame_keeps_queued_bytes_off_the_stream() {
        let mut frames = frame_bytes(true, true, 0x2, &standalone_compressed(FIRST));
        frames[0] |= 0x20;
        let mut socket = queued(frames);
        assert!(matches!(
            socket.read().unwrap_err(),
            Error::Protocol(ProtocolError::NonZeroReservedBits)
        ));
        assert!(matches!(socket.flush().unwrap_err(), Error::AlreadyClosed));
        nothing_reached_the_stream(&socket);
    }

    /// The same observation for the fragment-sequence caller, which is a different call
    /// site into the same helper and would otherwise be pinned only by reads.
    #[test]
    fn a_skipped_compressed_message_keeps_queued_bytes_off_the_stream() {
        let mut frames = frame_bytes(false, false, 0x2, b"open");
        frames.extend(frame_bytes(true, true, 0x2, &standalone_compressed(FIRST)));
        let mut socket = queued(frames);
        assert!(matches!(
            socket.read().unwrap_err(),
            Error::Protocol(ProtocolError::ExpectedFragment(_))
        ));
        assert!(matches!(socket.flush().unwrap_err(), Error::AlreadyClosed));
        nothing_reached_the_stream(&socket);
    }

    /// Without which any blanket latch passes the rows above. A rejection that discarded
    /// no compressed payload leaves the connection exactly as it was -- and the follow-up
    /// has to *decode*, not merely avoid `AlreadyClosed`: a message needing history the
    /// decoder lacks fails with `Compression`, which is the outcome this row exists to
    /// rule out and would otherwise have accepted.
    #[test]
    fn control_a_discarded_plain_frame_leaves_the_connection_open() {
        let mut frames = frame_bytes(true, false, 0x2, b"plain");
        frames[0] |= 0x20;
        frames.extend(frame_bytes(true, true, 0x2, &standalone_compressed(FIRST)));
        let mut socket = client(&frames);

        assert!(matches!(
            socket.read().unwrap_err(),
            Error::Protocol(ProtocolError::NonZeroReservedBits)
        ));
        assert_eq!(
            socket.read().expect("no compressed payload was discarded"),
            Message::binary(FIRST.to_vec()),
            "the connection is untouched, so the next message decodes exactly"
        );
    }

    /// A failed inflate has consumed part of the peer's stream and cannot be
    /// resynchronised, so the connection ends rather than decoding the next frame
    /// against a decoder that no longer describes it.
    #[test]
    fn a_decode_failure_ends_the_connection() {
        let mut socket = client(&[0xc2, 0x03, 0xff, 0xff, 0xff, 0xc2, 0x07]);
        assert!(matches!(socket.read().unwrap_err(), Error::Protocol(ProtocolError::Compression)));
        assert!(matches!(socket.read().unwrap_err(), Error::AlreadyClosed));
        assert!(matches!(
            socket.write(Message::binary(b"later".to_vec())).unwrap_err(),
            Error::AlreadyClosed
        ));
    }

    /// A message over `max_message_size` is the one decode error a caller may
    /// reasonably treat as recoverable, and it is not: the inflater stopped
    /// part-way through the peer's stream just the same.
    #[test]
    fn an_oversized_decompressed_message_ends_the_connection_too() {
        let mut frames = vec![0xc2, 0x07];
        frames.extend_from_slice(FIRST_FRAGMENT);
        frames.extend_from_slice(LAST_FRAGMENT);
        let config = WebSocketConfig::default().enable_deflate().max_message_size(Some(2));
        let mut socket =
            WebSocket::from_raw_socket(Incoming(Cursor::new(frames)), Role::Client, Some(config));
        assert!(matches!(
            socket.read().unwrap_err(),
            Error::Capacity(CapacityError::MessageTooLong { .. })
        ));
        assert!(matches!(socket.read().unwrap_err(), Error::AlreadyClosed));
    }

    /// Reads scripted frames and records everything that reaches the stream, so a row can
    /// assert that nothing did.
    struct Wire {
        inbound: Cursor<Vec<u8>>,
        written: Vec<u8>,
        flushes: usize,
    }

    impl Wire {
        fn new(inbound: Vec<u8>) -> Self {
            Self { inbound: Cursor::new(inbound), written: Vec::new(), flushes: 0 }
        }
    }

    impl io::Read for Wire {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            io::Read::read(&mut self.inbound, buf)
        }
    }

    impl io::Write for Wire {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    fn queued(inbound: Vec<u8>) -> WebSocket<Wire> {
        let config = WebSocketConfig::default()
            .enable_deflate()
            // larger than the queued frame, so `write` buffers instead of draining
            .write_buffer_size(64 * 1024);
        let mut socket = WebSocket::from_raw_socket(Wire::new(inbound), Role::Client, Some(config));
        socket.write(Message::binary(b"queued".to_vec())).expect("the frame queues");
        assert!(socket.get_ref().written.is_empty(), "the fixture must leave the frame queued");
        socket
    }

    /// Queue an outbound frame, then take an inbound inflate error, so each row below
    /// starts from a terminated codec with bytes still waiting to go out.
    fn queued_then_failed() -> WebSocket<Wire> {
        let mut socket = queued(vec![0xc2, 0x03, 0xff, 0xff, 0xff]);
        assert!(matches!(socket.read().unwrap_err(), Error::Protocol(ProtocolError::Compression)));
        socket
    }

    fn nothing_reached_the_stream(socket: &WebSocket<Wire>) {
        assert!(socket.get_ref().written.is_empty(), "no queued byte may reach the stream");
        assert_eq!(socket.get_ref().flushes, 0, "the stream must not be flushed");
    }

    /// `read` and `write` refuse a terminated connection on their own; queued bytes reach
    /// the stream through `flush`.
    #[test]
    fn a_codec_failure_keeps_queued_bytes_out_of_flush() {
        let mut socket = queued_then_failed();
        assert!(matches!(socket.flush().unwrap_err(), Error::AlreadyClosed));
        nothing_reached_the_stream(&socket);
    }

    /// `close` reaches the stream through `flush` too, and it needs its own failed socket:
    /// asserted after the flush row on a shared one, the flush assertion panics first on
    /// any tree without the fix and this arm is never observed at all.
    #[test]
    fn a_codec_failure_keeps_queued_bytes_out_of_close() {
        let mut socket = queued_then_failed();
        assert!(matches!(socket.close(None).unwrap_err(), Error::AlreadyClosed));
        nothing_reached_the_stream(&socket);
    }

    /// Without which the row above passes on a fixture that never had anything to write.
    #[test]
    fn control_the_same_queued_bytes_are_flushed_when_the_codec_is_healthy() {
        let mut socket = queued(Vec::new());
        socket.flush().expect("a healthy connection flushes");
        assert!(!socket.get_ref().written.is_empty(), "the queued frame reaches the stream");
        assert_eq!(socket.get_ref().flushes, 1);
    }

    /// Compression history is private, so a caller cannot keep it in step with a
    /// frame it compressed itself; under context takeover the next ordinary
    /// message would reference a history the peer does not have. The escape hatch
    /// for raw frames stays open as long as the bit is clear.
    #[test]
    fn a_caller_owned_frame_may_not_claim_rsv1_while_deflate_is_negotiated() {
        let mut socket = WebSocket::from_raw_socket(
            Incoming(Cursor::new(Vec::new())),
            Role::Server,
            Some(WebSocketConfig::default().enable_deflate()),
        );
        let mut frame = Frame::message(vec![b'x'; 4], OpCode::Data(OpData::Binary), true);
        frame.header_mut().rsv1 = true;
        assert!(matches!(
            socket.write(Message::Frame(frame)).unwrap_err(),
            Error::Protocol(ProtocolError::NonZeroReservedBits)
        ));

        let frame = Frame::message(vec![b'x'; 4], OpCode::Data(OpData::Binary), true);
        socket.write(Message::Frame(frame)).expect("an rsv1-clear raw frame still queues");
    }
}

#[cfg(all(test, feature = "deflate"))]
mod write_transaction {
    use super::*;
    use crate::Message;
    use std::io;

    /// A stream that keeps what was written, so a peer can decode it.
    #[derive(Default)]
    struct Recorder(Vec<u8>);

    impl io::Write for Recorder {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl io::Read for Recorder {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    /// Sized so one message fits the write buffer and a second does not, which is
    /// the only shape where a rejected write is retryable: a frame larger than
    /// `max_write_buffer_size` fails admission on every retry forever.
    fn server() -> WebSocket<Recorder> {
        let config = WebSocketConfig::default()
            // `write_buffer_size` must exceed the first message, or `write`
            // auto-flushes it and the buffer is empty when the second arrives --
            // in which case nothing is ever rejected and every arm is vacuous.
            .write_buffer_size(400)
            .max_write_buffer_size(500)
            .enable_deflate();
        WebSocket::from_raw_socket(Recorder::default(), Role::Server, Some(config))
    }

    /// Poorly compressible, so the buffer fills with real bytes. A compressible
    /// filler would leave the buffer nearly empty while the preflight measures the
    /// *plain* frame, and nothing would ever be rejected.
    fn noise(len: usize, seed: u8) -> Vec<u8> {
        // splitmix64. A linear sequence is not incompressible -- deflate takes 300
        // bytes of `i * 37 + seed` down to 276, which would silently defeat the
        // sizing every arm depends on.
        let mut x = u64::from(seed).wrapping_add(0x9E37_79B9_7F4A_7C15);
        (0..len)
            .map(|_| {
                x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = x;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                (z ^ (z >> 31)) as u8
            })
            .collect()
    }

    /// Decode a recorded stream, refusing to mistake a decode failure for its end.
    ///
    /// Running out of bytes is the only acceptable way to stop; anything else means the
    /// wire we produced does not decode, which is what these rows exist to detect. A loop
    /// that stops on the first `Err` accepts the expected prefix followed by any garbage.
    fn decode_all(wire: &[u8]) -> Vec<Message> {
        let config = WebSocketConfig::default().enable_deflate();
        let mut peer =
            WebSocket::from_raw_socket(io::Cursor::new(wire.to_vec()), Role::Client, Some(config));
        let mut out = Vec::new();
        loop {
            match peer.read() {
                Ok(message) => out.push(message),
                Err(Error::Protocol(ProtocolError::ResetWithoutClosingHandshake)) => return out,
                Err(other) => panic!("the recorded wire failed to decode: {other:?}"),
            }
        }
    }

    /// Fills the buffer, then offers a message the preflight must reject.
    /// Returns the socket and the frame handed back.
    fn reject_one(first: &[u8], second: &[u8]) -> (WebSocket<Recorder>, Frame) {
        let mut socket = server();
        socket.write(Message::binary(first.to_vec())).expect("the first message fits");

        let returned = match socket.write(Message::binary(second.to_vec())) {
            Err(Error::WriteBufferFull(message)) => match *message {
                Message::Frame(frame) => frame,
                other => panic!("WriteBufferFull must carry a frame, got {other:?}"),
            },
            other => panic!("the second message must be rejected, got {other:?}"),
        };
        assert!(
            !returned.header().rsv1,
            "the returned frame must be uncompressed, which is what makes every retry order safe"
        );
        (socket, returned)
    }

    /// The expansion branch: the plain frame fits, the compressed one does not.
    ///
    /// Incompressible input inflates slightly under deflate, so the window between
    /// the two sizes is only a handful of bytes. Rather than guess it, compress the
    /// payload with a throwaway context to learn the compressed length, then set
    /// `max_write_buffer_size` to exactly the plain wire size — the preflight
    /// admits it, the compressed frame overflows, and the branch fires.
    ///
    /// The reset in that branch is what this arm exists for. Without it the encoder
    /// keeps a window containing a message the peer never inflated, because that
    /// message went out uncompressed, and every later compressed frame references
    /// history the peer does not have.
    #[test]
    fn arm_four_expansion_sends_plain_and_leaves_the_next_message_decodable() {
        use crate::protocol::deflate::{Context, Settings};

        let payload = noise(300, 7);
        let compressed_len = Context::new(Role::Server, Settings::default())
            .compress(&payload)
            .expect("compress")
            .len();
        assert!(
            compressed_len > payload.len(),
            "the fixture must actually expand: {} -> {compressed_len}",
            payload.len()
        );

        let plain_wire = Frame::message(payload.clone(), OpCode::Data(OpData::Binary), true).len();
        let config = WebSocketConfig::default()
            .write_buffer_size(plain_wire - 1)
            .max_write_buffer_size(plain_wire)
            .enable_deflate();
        let mut socket =
            WebSocket::from_raw_socket(Recorder::default(), Role::Server, Some(config));

        socket
            .write(Message::binary(payload.clone()))
            .expect("the plain frame fits, so this must not be rejected");
        socket.flush().expect("flush");

        let after_expansion = socket.get_ref().0.len();
        let first = decode_all(&socket.get_ref().0);
        assert_eq!(
            first,
            vec![Message::binary(payload.clone())],
            "the expansion branch must send the message uncompressed"
        );

        // A second message, now compressed, against a peer whose inflate window
        // never saw the first. It has to *share bytes* with the first, or the
        // encoder finds nothing to back-reference and a stale window is
        // indistinguishable from a fresh one.
        let second = payload[..120].to_vec();
        socket.write(Message::binary(second.clone())).expect("second send");
        socket.flush().expect("flush");

        let tail = &socket.get_ref().0[after_expansion..];
        assert_eq!(
            decode_all(tail),
            vec![Message::binary(second)],
            "without the expansion reset the encoder references history the peer \
             never received, and this decodes to garbage or fails"
        );
    }

    /// Admission is decided on the *uncompressed* size, which is what makes the
    /// `max_write_buffer_size` contract true: a message whose plain wire form does
    /// not fit is rejected even when its compressed form would have.
    ///
    /// This is the arm the incompressible fillers cannot reach. With noise, plain
    /// and compressed both miss admission, so the expansion reset produces the same
    /// outcome as the preflight and deleting the preflight is invisible. With a
    /// highly compressible payload the two decisions disagree, and only the
    /// preflight rejects.
    #[test]
    fn arm_five_a_large_compressible_message_is_rejected_on_its_plain_size() {
        let payload = vec![b'z'; 400];
        let plain_wire = Frame::message(payload.clone(), OpCode::Data(OpData::Binary), true).len();
        let compressed_len = crate::protocol::deflate::Context::new(
            Role::Server,
            crate::protocol::deflate::Settings::default(),
        )
        .compress(&payload)
        .expect("compress")
        .len();
        assert!(
            compressed_len + 4 < 200 && plain_wire > 200,
            "the fixture must straddle the cap: plain {plain_wire}, compressed {compressed_len}"
        );

        let config = WebSocketConfig::default()
            .write_buffer_size(100)
            .max_write_buffer_size(200)
            .enable_deflate();
        let mut socket =
            WebSocket::from_raw_socket(Recorder::default(), Role::Server, Some(config));

        match socket.write(Message::binary(payload.clone())) {
            Err(Error::WriteBufferFull(message)) => match *message {
                Message::Frame(frame) => {
                    assert!(!frame.header().rsv1, "the returned frame must be the uncompressed one")
                }
                other => panic!("must carry a frame, got {other:?}"),
            },
            other => panic!(
                "a message whose plain form exceeds the cap must be rejected even though \
                 it compresses small enough to fit -- got {other:?}"
            ),
        }
        socket.flush().expect("flush");
        assert!(socket.get_ref().0.is_empty(), "nothing may reach the wire for a rejected message");
    }

    /// The client mask is not present on `plain` yet, but its four bytes belong
    /// to the preflight size. The cap sits inside that exact gap: estimating this
    /// client frame as a server frame admits it, after which compression makes it
    /// small enough for the later check and silently changes the documented
    /// uncompressed-size decision.
    #[test]
    fn arm_six_client_preflight_counts_the_mask_before_it_is_added() {
        let payload = vec![b'z'; 400];
        let plain = Frame::message(payload.clone(), OpCode::Data(OpData::Binary), true);
        let server_wire = wire_size(Role::Server, &plain);
        let client_wire = wire_size(Role::Client, &plain);
        assert_eq!(client_wire, server_wire + 4, "the fixture must isolate the mask term");
        let cap = server_wire + 2;

        let config = WebSocketConfig::default()
            .write_buffer_size(100)
            .max_write_buffer_size(cap)
            .enable_deflate();
        let mut socket =
            WebSocket::from_raw_socket(Recorder::default(), Role::Client, Some(config));

        match socket.write(Message::binary(payload)) {
            Err(Error::WriteBufferFull(message)) => match *message {
                Message::Frame(frame) => {
                    assert!(!frame.header().rsv1, "the returned frame must be uncompressed")
                }
                other => panic!("must carry a frame, got {other:?}"),
            },
            other => panic!("the client mask makes the plain frame exceed the cap: {other:?}"),
        }
    }

    /// The compressed-size check has its own role-bearing call site. An expanding
    /// payload makes the plain client frame fit while the compressed client frame
    /// crosses the cap only because of its mask. Estimating that second frame as
    /// a server frame returns it as compressed; the later retry admission then
    /// rejects it instead of the expansion branch sending the safe plain frame.
    #[test]
    fn arm_seven_client_compressed_size_counts_the_mask() {
        use crate::protocol::deflate::{Context, Settings};

        let payload = noise(300, 7);
        let compressed =
            Context::new(Role::Client, Settings::default()).compress(&payload).expect("compress");
        assert!(compressed.len() > payload.len(), "the fixture must expand");

        let plain = Frame::message(payload.clone(), OpCode::Data(OpData::Binary), true);
        let mut compressed_frame = Frame::message(compressed, OpCode::Data(OpData::Binary), true);
        compressed_frame.header_mut().rsv1 = true;
        let plain_wire = wire_size(Role::Client, &plain);
        let compressed_server_wire = wire_size(Role::Server, &compressed_frame);
        let compressed_client_wire = wire_size(Role::Client, &compressed_frame);
        assert_eq!(compressed_client_wire, compressed_server_wire + 4);
        let cap = compressed_server_wire + 2;
        assert!(
            plain_wire <= cap && cap < compressed_client_wire,
            "the cap must isolate the compressed-frame mask term"
        );

        let config = WebSocketConfig::default()
            .write_buffer_size(0)
            .max_write_buffer_size(cap)
            .enable_deflate();
        let mut socket =
            WebSocket::from_raw_socket(Recorder::default(), Role::Client, Some(config));
        socket
            .write(Message::binary(payload.clone()))
            .expect("expansion must fall back to the plain frame");
        socket.flush().expect("flush");

        let wire = &socket.get_ref().0;
        assert_eq!(wire[0] & 0x40, 0, "the expansion branch must clear RSV1");
        let mut peer = WebSocket::from_raw_socket(
            io::Cursor::new(wire.clone()),
            Role::Server,
            Some(WebSocketConfig::default().enable_deflate()),
        );
        assert_eq!(peer.read().expect("peer reads the plain frame"), Message::binary(payload));
    }

    #[test]
    fn arm_one_immediate_retry_of_the_returned_frame() {
        let (a, b) = (noise(300, 1), noise(300, 2));
        let (mut socket, returned) = reject_one(&a, &b);
        socket.flush().expect("draining makes room");
        socket.write(Message::Frame(returned)).expect("the returned frame now fits");
        socket.flush().expect("flush");

        let decoded = decode_all(&socket.get_ref().0);
        assert_eq!(decoded, vec![Message::binary(a.clone()), Message::binary(b.clone())]);
    }

    #[test]
    fn arm_two_drop_the_returned_frame_then_send_another() {
        let (a, b) = (noise(300, 1), noise(300, 2));
        let (mut socket, returned) = reject_one(&a, &b);
        drop(returned);
        socket.flush().expect("flush");
        let c = noise(150, 3);
        socket.write(Message::binary(c.clone())).expect("a later message still sends");
        socket.flush().expect("flush");

        let decoded = decode_all(&socket.get_ref().0);
        assert_eq!(
            decoded,
            vec![Message::binary(a.clone()), Message::binary(c.clone())],
            "dropping the rejected message must not corrupt what follows"
        );
    }

    #[test]
    fn arm_three_another_message_first_then_retry_the_returned_frame() {
        let (a, b) = (noise(300, 1), noise(300, 2));
        let (mut socket, returned) = reject_one(&a, &b);
        socket.flush().expect("flush");
        let c = noise(150, 3);
        socket.write(Message::binary(c.clone())).expect("send an intervening message");
        socket.flush().expect("flush");
        socket.write(Message::Frame(returned)).expect("retry after C");
        socket.flush().expect("flush");

        let decoded = decode_all(&socket.get_ref().0);
        assert_eq!(
            decoded,
            vec![
                Message::binary(a.clone()),
                Message::binary(c.clone()),
                Message::binary(b.clone())
            ],
            "the returned frame is uncompressed, so an intervening compressed \
             message cannot shift its back-references"
        );
    }

    /// The raw-frame path hands back a frame that is *already masked* -- the one shape
    /// `wire_size` must never be given, because it would count the mask twice.
    ///
    /// `buffer_frame` masks a client frame before its own capacity check, so the frame
    /// inside `WriteBufferFull` is four bytes longer than the one the caller passed in.
    /// Nothing asserted that in either direction, and the `debug_assert!` in `wire_size`
    /// is only worth its line while it holds.
    ///
    /// The cap sits one byte above the unmasked frame, so admission turns on the mask
    /// alone: the server writes the same frame under the same cap.
    #[test]
    fn a_rejected_client_raw_frame_is_returned_masked() {
        let frame = Frame::message(vec![b'z'; 400], OpCode::Data(OpData::Binary), true);
        let unmasked = frame.len();
        let config = WebSocketConfig::default()
            .write_buffer_size(0)
            .max_write_buffer_size(unmasked + 1)
            .enable_deflate();

        let mut server =
            WebSocket::from_raw_socket(Recorder::default(), Role::Server, Some(config));
        server.write(Message::Frame(frame.clone())).expect("a server frame fits under this cap");

        let mut client =
            WebSocket::from_raw_socket(Recorder::default(), Role::Client, Some(config));
        match client.write(Message::Frame(frame)) {
            Err(Error::WriteBufferFull(message)) => match *message {
                Message::Frame(returned) => {
                    assert!(
                        returned.header().mask.is_some(),
                        "the capacity check runs after masking, so the frame comes back masked"
                    );
                    assert_eq!(
                        returned.len(),
                        unmasked + 4,
                        "and it is four bytes longer than the frame handed in"
                    );
                }
                other => panic!("must carry a frame, got {other:?}"),
            },
            other => panic!("the mask is what pushes this frame past the cap: {other:?}"),
        }
    }
}

#[cfg(all(test, feature = "deflate"))]
mod wire_size_tests {
    use super::*;
    use crate::protocol::frame::coding::{Data as OpData, OpCode};

    /// The preflight admits a message by comparing `wire_size` against the write
    /// buffer *before* compressing, so this number has to be the size the frame
    /// will actually occupy — not the size it occupies now.
    ///
    /// The load-bearing term is the mask: a client frame is masked later, in
    /// `buffer_frame`, so at preflight time it is four bytes smaller than it will
    /// be on the wire. Undercounting by four would let a client overflow a bound
    /// its caller chose, which is the whole thing the preflight exists to prevent.
    ///
    /// Lengths are the header-format boundaries: 0/125 take a 2-byte header, 126
    /// and 65535 take 4, and 65536 takes 10.
    #[test]
    fn client_counts_the_mask_it_has_not_added_yet_at_every_header_boundary() {
        for (payload, header) in [(0, 2), (125, 2), (126, 4), (65_535, 4), (65_536, 10)] {
            let frame = Frame::message(vec![0u8; payload], OpCode::Data(OpData::Binary), true);
            assert!(!frame.is_masked(), "a fresh frame is unmasked");
            assert_eq!(frame.len(), header + payload, "payload {payload}");

            assert_eq!(
                wire_size(Role::Server, &frame),
                header + payload,
                "a server never masks, so wire size is the frame ({payload})"
            );
            assert_eq!(
                wire_size(Role::Client, &frame),
                header + payload + 4,
                "a client masks at buffer time, so preflight must add 4 ({payload})"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Message, Role, WebSocket, WebSocketConfig};
    use crate::error::{CapacityError, Error};

    use std::{io, io::Cursor};

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
        let mut socket = WebSocket::from_raw_socket(WriteMoc(incoming), Role::Client, None);
        assert_eq!(socket.read().unwrap(), Message::Ping(vec![1, 2].into()));
        assert_eq!(socket.read().unwrap(), Message::Pong(vec![3].into()));
        assert_eq!(socket.read().unwrap(), Message::Text("Hello, World!".into()));
        assert_eq!(socket.read().unwrap(), Message::Binary(vec![0x01, 0x02, 0x03].into()));
    }

    #[test]
    fn size_limiting_text_fragmented() {
        let incoming = Cursor::new(vec![
            0x01, 0x07, 0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x2c, 0x20, 0x80, 0x06, 0x57, 0x6f, 0x72,
            0x6c, 0x64, 0x21,
        ]);
        let limit = WebSocketConfig { max_message_size: Some(10), ..WebSocketConfig::default() };
        let mut socket = WebSocket::from_raw_socket(WriteMoc(incoming), Role::Client, Some(limit));

        assert!(matches!(
            socket.read(),
            Err(Error::Capacity(CapacityError::MessageTooLong { size: 13, max_size: 10 }))
        ));
    }

    #[test]
    fn size_limiting_binary() {
        let incoming = Cursor::new(vec![0x82, 0x03, 0x01, 0x02, 0x03]);
        let limit = WebSocketConfig { max_message_size: Some(2), ..WebSocketConfig::default() };
        let mut socket = WebSocket::from_raw_socket(WriteMoc(incoming), Role::Client, Some(limit));

        assert!(matches!(
            socket.read(),
            Err(Error::Capacity(CapacityError::MessageTooLong { size: 3, max_size: 2 }))
        ));
    }

    #[test]
    fn set_config_changes_the_configuration() {
        let mut socket =
            WebSocket::from_raw_socket(WriteMoc(Cursor::new(Vec::<u8>::new())), Role::Client, None);
        assert_eq!(socket.get_config().max_message_size, Some(64 << 20));

        socket.set_config(|config| config.max_message_size = Some(1024));

        assert_eq!(socket.get_config().max_message_size, Some(1024));
    }

    /// The feature-on arm applies the callback to a copy, so what survives a panic depends
    /// on which panic it is. Validation still fires after the commit, as upstream does.
    #[cfg(feature = "deflate")]
    #[test]
    fn set_config_panics_leave_the_live_config_in_a_defined_state() {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        fn agreed_socket() -> WebSocket<WriteMoc<Cursor<Vec<u8>>>> {
            WebSocket::from_raw_socket(
                WriteMoc(Cursor::new(Vec::<u8>::new())),
                Role::Client,
                Some(WebSocketConfig::default().enable_deflate()),
            )
        }

        let mut invalid = agreed_socket();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            invalid.set_config(|config| config.max_write_buffer_size = config.write_buffer_size)
        }));
        assert!(outcome.is_err(), "an invalid candidate must still be rejected");
        assert_eq!(
            invalid.get_config().max_write_buffer_size,
            invalid.get_config().write_buffer_size,
            "upstream commits before validating, and the deflate arm keeps that ordering"
        );

        let mut callback = agreed_socket();
        let untouched = callback.get_config().max_message_size;
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            callback.set_config(|config| {
                config.max_message_size = Some(1);
                panic!("the callback itself fails");
            })
        }));
        assert!(outcome.is_err(), "the callback's own panic must propagate");
        assert_eq!(
            callback.get_config().max_message_size,
            untouched,
            "the candidate is discarded, so the callback's edit never lands"
        );

        // An agreed `Some(..)` moving to a different `Some(..)`. Starting from `None` also
        // panics, while measuring that deflate cannot be switched on mid-connection --
        // a different claim from the one this assert makes.
        let mut agreed = agreed_socket();
        let settled = agreed.get_config().deflate;
        assert!(settled.is_some(), "the socket must start from an agreed Some(..)");
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            agreed.set_config(|config| *config = config.deflate_max_window_bits(Role::Client, 10))
        }));
        let message = *outcome
            .expect_err("a deflate change must be rejected")
            .downcast::<String>()
            .expect("assert_eq! panics with a String");
        assert!(message.contains("agreed deflate settings are immutable"), "{message}");
        assert_eq!(agreed.get_config().deflate, settled, "rejection happens before the commit");
    }
}
