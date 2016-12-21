use std::convert::{From, Into, AsRef};
use std::fmt;
use std::result::Result as StdResult;
use std::str;

use error::Result;

mod string_collect {

    use utf8;

    use error::{Error, Result};

    pub struct StringCollector {
        data: String,
        decoder: utf8::Decoder,
    }

    impl StringCollector {
        pub fn new() -> Self {
            StringCollector {
                data: String::new(),
                decoder: utf8::Decoder::new(),
            }
        }
        pub fn extend<T: AsRef<[u8]>>(&mut self, tail: T) -> Result<()> {
            let (sym, text, result) = self.decoder.decode(tail.as_ref());
            self.data.push_str(&sym);
            self.data.push_str(text);
            match result {
                utf8::Result::Ok | utf8::Result::Incomplete =>
                    Ok(()),
                utf8::Result::Error { remaining_input_after_error: _ } =>
                    Err(Error::Protocol("Invalid UTF8".into())), // FIXME
            }
        }
        pub fn into_string(self) -> Result<String> {
            if self.decoder.has_incomplete_sequence() {
                Err(Error::Protocol("Invalid UTF8".into())) // FIXME
            } else {
                Ok(self.data)
            }
        }
    }

}

use self::string_collect::StringCollector;

/// A struct representing the incomplete message.
pub struct IncompleteMessage {
    collector: IncompleteMessageCollector,
}

enum IncompleteMessageCollector {
    Text(StringCollector),
    Binary(Vec<u8>),
}

impl IncompleteMessage {
    /// Create new.
    pub fn new(message_type: IncompleteMessageType) -> Self {
        IncompleteMessage {
            collector: match message_type {
                IncompleteMessageType::Binary =>
                    IncompleteMessageCollector::Binary(Vec::new()),
                IncompleteMessageType::Text =>
                    IncompleteMessageCollector::Text(StringCollector::new()),
            }
        }
    }
    /// Add more data to an existing message.
    pub fn extend<T: AsRef<[u8]>>(&mut self, tail: T) -> Result<()> {
        match self.collector {
            IncompleteMessageCollector::Binary(ref mut v) => {
                v.extend(tail.as_ref());
                Ok(())
            }
            IncompleteMessageCollector::Text(ref mut t) => {
                t.extend(tail)
            }
        }
    }
    /// Convert an incomplete message into a complete one.
    pub fn complete(self) -> Result<Message> {
        match self.collector {
            IncompleteMessageCollector::Binary(v) => {
                Ok(Message::Binary(v))
            }
            IncompleteMessageCollector::Text(t) => {
                let text = t.into_string()?;
                Ok(Message::Text(text))
            }
        }
    }
}

/// The type of incomplete message.
pub enum IncompleteMessageType {
    Text,
    Binary,
}

/// An enum representing the various forms of a WebSocket message.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum Message {
    /// A text WebSocket message
    Text(String),
    /// A binary WebSocket message
    Binary(Vec<u8>),
}

impl Message {

    /// Create a new text WebSocket message from a stringable.
    pub fn text<S>(string: S) -> Message
        where S: Into<String>
    {
        Message::Text(string.into())
    }

    /// Create a new binary WebSocket message by converting to Vec<u8>.
    pub fn binary<B>(bin: B) -> Message
        where B: Into<Vec<u8>>
    {
        Message::Binary(bin.into())
    }

    /// Indicates whether a message is a text message.
    pub fn is_text(&self) -> bool {
        match *self {
            Message::Text(_) => true,
            Message::Binary(_) => false,
        }
    }

    /// Indicates whether a message is a binary message.
    pub fn is_binary(&self) -> bool {
        match *self {
            Message::Text(_) => false,
            Message::Binary(_) => true,
        }
    }

    /// Get the length of the WebSocket message.
    pub fn len(&self) -> usize {
        match *self {
            Message::Text(ref string) => string.len(),
            Message::Binary(ref data) => data.len(),
        }
    }

    /// Returns true if the WebSocket message has no content.
    /// For example, if the other side of the connection sent an empty string.
    pub fn is_empty(&self) -> bool {
        match *self {
            Message::Text(ref string) => string.is_empty(),
            Message::Binary(ref data) => data.is_empty(),
        }
    }

    /// Consume the WebSocket and return it as binary data.
    pub fn into_data(self) -> Vec<u8> {
        match self {
            Message::Text(string) => string.into_bytes(),
            Message::Binary(data) => data,
        }
    }

    /// Attempt to consume the WebSocket message and convert it to a String.
    pub fn into_text(self) -> Result<String> {
        match self {
            Message::Text(string) => Ok(string),
            Message::Binary(data) => Ok(try!(
                String::from_utf8(data).map_err(|err| err.utf8_error()))),
        }
    }

    /// Attempt to get a &str from the WebSocket message,
    /// this will try to convert binary data to utf8.
    pub fn to_text(&self) -> Result<&str> {
        match *self {
            Message::Text(ref string) => Ok(string),
            Message::Binary(ref data) => Ok(try!(str::from_utf8(data))),
        }
    }

}

impl From<String> for Message {
    fn from(string: String) -> Message {
        Message::text(string)
    }
}

impl<'s> From<&'s str> for Message {
    fn from(string: &'s str) -> Message {
        Message::text(string)
    }
}

impl<'b> From<&'b [u8]> for Message {
    fn from(data: &'b [u8]) -> Message {
        Message::binary(data)
    }
}

impl From<Vec<u8>> for Message {
    fn from(data: Vec<u8>) -> Message {
        Message::binary(data)
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter) -> StdResult<(), fmt::Error> {
        if let Ok(string) = self.to_text() {
            write!(f, "{}", string)
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
        let t = Message::text(format!("test"));
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
    fn binary_convert_vec() {
        let bin = vec![6u8, 7, 8, 9, 10, 241];
        let msg = Message::from(bin);
        assert!(msg.is_binary());
        assert!(msg.into_text().is_err());
    }

    #[test]
    fn text_convert() {
        let s = "kiwotsukete";
        let msg = Message::from(s);
        assert!(msg.is_text());
    }
}
