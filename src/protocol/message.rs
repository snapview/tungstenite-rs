use super::frame::{CloseFrame, Frame};
use crate::{
    error::{CapacityError, Error, Result},
    protocol::frame::Utf8Bytes,
};
use bytes::{Bytes, BytesMut};
use std::{fmt, result::Result as StdResult, str};

/// A struct representing the incomplete message.
///
/// Note: Text messages are utf8 validated on calling [`Self::complete`].
#[derive(Debug)]
pub(crate) struct IncompleteMessage {
    kind: MessageType,
    buf: BytesMut,
}

impl IncompleteMessage {
    pub fn new(kind: MessageType) -> Self {
        Self { kind, buf: BytesMut::new() }
    }

    /// Add more data to an existing message.
    pub fn extend(&mut self, tail: Bytes, size_limit: Option<usize>) -> Result<()> {
        // Always have a max size. This ensures an error in case of concatenating two buffers
        // of more than `usize::MAX` bytes in total.
        let max_size = size_limit.unwrap_or(usize::MAX);
        let my_size = self.buf.len();
        let portion_size = tail.len();
        // Be careful about integer overflows here.
        if my_size > max_size || portion_size > max_size - my_size {
            return Err(Error::Capacity(CapacityError::MessageTooLong {
                size: my_size.saturating_add(portion_size),
                max_size,
            }));
        }

        self.buf.extend_from_slice(&tail);
        Ok(())
    }

    /// Convert an incomplete message into a complete one.
    pub fn complete(self) -> Result<Message> {
        Ok(match self.kind {
            MessageType::Binary => Message::Binary(self.buf.freeze()),
            MessageType::Text => Message::Text(self.buf.try_into()?),
        })
    }
}

/// The type of incomplete message.
#[derive(Debug)]
pub enum MessageType {
    Text,
    Binary,
}

/// An enum representing the various forms of a WebSocket message.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum Message {
    /// A text WebSocket message
    Text(Utf8Bytes),
    /// A binary WebSocket message
    Binary(Bytes),
    /// A ping message with the specified payload
    ///
    /// The payload here must have a length less than 125 bytes
    Ping(Bytes),
    /// A pong message with the specified payload
    ///
    /// The payload here must have a length less than 125 bytes
    Pong(Bytes),
    /// A close message with the optional close frame.
    Close(Option<CloseFrame>),
    /// Raw frame. Note, that you're not going to get this value while reading the message.
    Frame(Frame),
}

impl Message {
    /// Create a new text WebSocket message from a stringable.
    pub fn text<S>(string: S) -> Message
    where
        S: Into<Utf8Bytes>,
    {
        Message::Text(string.into())
    }

    /// Create a new binary WebSocket message by converting to `Bytes`.
    pub fn binary<B>(bin: B) -> Message
    where
        B: Into<Bytes>,
    {
        Message::Binary(bin.into())
    }

    /// Indicates whether a message is a text message.
    pub fn is_text(&self) -> bool {
        matches!(*self, Message::Text(_))
    }

    /// Indicates whether a message is a binary message.
    pub fn is_binary(&self) -> bool {
        matches!(*self, Message::Binary(_))
    }

    /// Indicates whether a message is a ping message.
    pub fn is_ping(&self) -> bool {
        matches!(*self, Message::Ping(_))
    }

    /// Indicates whether a message is a pong message.
    pub fn is_pong(&self) -> bool {
        matches!(*self, Message::Pong(_))
    }

    /// Indicates whether a message is a close message.
    pub fn is_close(&self) -> bool {
        matches!(*self, Message::Close(_))
    }

    /// Get the length of the WebSocket message.
    pub fn len(&self) -> usize {
        match *self {
            Message::Text(ref string) => string.len(),
            Message::Binary(ref data) | Message::Ping(ref data) | Message::Pong(ref data) => {
                data.len()
            }
            Message::Close(ref data) => data.as_ref().map(|d| d.reason.len()).unwrap_or(0),
            Message::Frame(ref frame) => frame.len(),
        }
    }

    /// Returns true if the WebSocket message has no content.
    /// For example, if the other side of the connection sent an empty string.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Consume the WebSocket and return it as binary data.
    pub fn into_data(self) -> Bytes {
        match self {
            Message::Text(utf8) => utf8.into(),
            Message::Binary(data) | Message::Ping(data) | Message::Pong(data) => data,
            Message::Close(None) => <_>::default(),
            Message::Close(Some(frame)) => frame.reason.into(),
            Message::Frame(frame) => frame.into_payload(),
        }
    }

    /// Attempt to consume the WebSocket message and convert it to a String.
    pub fn into_text(self) -> Result<Utf8Bytes> {
        match self {
            Message::Text(txt) => Ok(txt),
            Message::Binary(data) | Message::Ping(data) | Message::Pong(data) => {
                Ok(data.try_into()?)
            }
            Message::Close(None) => Ok(<_>::default()),
            Message::Close(Some(frame)) => Ok(frame.reason),
            Message::Frame(frame) => Ok(frame.into_text()?),
        }
    }

    /// Attempt to get a &str from the WebSocket message,
    /// this will try to convert binary data to utf8.
    pub fn to_text(&self) -> Result<&str> {
        match *self {
            Message::Text(ref string) => Ok(string.as_str()),
            Message::Binary(ref data) | Message::Ping(ref data) | Message::Pong(ref data) => {
                Ok(str::from_utf8(data)?)
            }
            Message::Close(None) => Ok(""),
            Message::Close(Some(ref frame)) => Ok(&frame.reason),
            Message::Frame(ref frame) => Ok(frame.to_text()?),
        }
    }
}

impl From<String> for Message {
    #[inline]
    fn from(string: String) -> Self {
        Message::text(string)
    }
}

impl<'s> From<&'s str> for Message {
    #[inline]
    fn from(string: &'s str) -> Self {
        Message::text(string)
    }
}

impl<'b> From<&'b [u8]> for Message {
    #[inline]
    fn from(data: &'b [u8]) -> Self {
        Message::binary(Bytes::copy_from_slice(data))
    }
}

impl From<Bytes> for Message {
    fn from(data: Bytes) -> Self {
        Message::binary(data)
    }
}

impl From<Vec<u8>> for Message {
    #[inline]
    fn from(data: Vec<u8>) -> Self {
        Message::binary(data)
    }
}

impl From<Message> for Bytes {
    #[inline]
    fn from(message: Message) -> Self {
        message.into_data()
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter) -> StdResult<(), fmt::Error> {
        if let Ok(string) = self.to_text() {
            write!(f, "{string}")
        } else {
            write!(f, "Binary Data<length={}>", self.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display() {
        let t = Message::text("test".to_owned());
        assert_eq!(t.to_string(), "test".to_owned());

        let bin = Message::binary(vec![0, 1, 3, 4, 241]);
        assert_eq!(bin.to_string(), "Binary Data<length=5>".to_owned());
    }

    #[test]
    fn binary_convert() {
        let bin = [6u8, 7, 8, 9, 10, 241];
        let msg = Message::from(&bin[..]);
        assert!(msg.is_binary());
        assert!(msg.into_text().is_err());
    }

    #[test]
    fn binary_convert_bytes() {
        let bin = Bytes::from_iter([6u8, 7, 8, 9, 10, 241]);
        let msg = Message::from(bin);
        assert!(msg.is_binary());
        assert!(msg.into_text().is_err());
    }

    #[test]
    fn binary_convert_vec() {
        let bin = vec![6u8, 7, 8, 9, 10, 241];
        let msg = Message::from(bin);
        assert!(msg.is_binary());
        assert!(msg.into_text().is_err());
    }

    #[test]
    fn binary_convert_into_bytes() {
        let bin = vec![6u8, 7, 8, 9, 10, 241];
        let bin_copy = bin.clone();
        let msg = Message::from(bin);
        let serialized: Bytes = msg.into();
        assert_eq!(bin_copy, serialized);
    }

    #[test]
    fn text_convert() {
        let s = "kiwotsukete";
        let msg = Message::from(s);
        assert!(msg.is_text());
    }
}
