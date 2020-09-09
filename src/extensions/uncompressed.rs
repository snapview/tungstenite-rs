use crate::extensions::WebSocketExtension;
use crate::protocol::frame::coding::{Data, OpCode};
use crate::protocol::frame::Frame;
use crate::protocol::message::{IncompleteMessage, IncompleteMessageType};
use crate::protocol::MAX_MESSAGE_SIZE;
use crate::{Error, Message};

#[derive(Debug)]
pub struct PlainTextExt {
    incomplete: Option<IncompleteMessage>,
    max_message_size: Option<usize>,
}

impl PlainTextExt {
    pub fn new(max_message_size: Option<usize>) -> PlainTextExt {
        PlainTextExt {
            incomplete: None,
            max_message_size,
        }
    }
}

impl Clone for PlainTextExt {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl Default for PlainTextExt {
    fn default() -> Self {
        PlainTextExt {
            incomplete: None,
            max_message_size: Some(MAX_MESSAGE_SIZE),
        }
    }
}

impl WebSocketExtension for PlainTextExt {
    type Error = Error;

    fn enabled(&self) -> bool {
        true
    }

    fn rsv1(&self) -> bool {
        false
    }

    fn on_receive_frame(&mut self, frame: Frame) -> Result<Option<Message>, Self::Error> {
        let fin = frame.header().is_final;

        match frame.header().opcode {
            OpCode::Data(data) => match data {
                Data::Continue => {
                    if let Some(ref mut msg) = self.incomplete {
                        msg.extend(frame.into_data(), self.max_message_size)?;
                    } else {
                        return Err(Error::Protocol(
                            "Continue frame but nothing to continue".into(),
                        ));
                    }
                    if fin {
                        Ok(Some(self.incomplete.take().unwrap().complete()?))
                    } else {
                        Ok(None)
                    }
                }
                c if self.incomplete.is_some() => Err(Error::Protocol(
                    format!("Received {} while waiting for more fragments", c).into(),
                )),
                Data::Text | Data::Binary => {
                    let msg = {
                        let message_type = match data {
                            Data::Text => IncompleteMessageType::Text,
                            Data::Binary => IncompleteMessageType::Binary,
                            _ => panic!("Bug: message is not text nor binary"),
                        };
                        let mut m = IncompleteMessage::new(message_type);
                        m.extend(frame.into_data(), self.max_message_size)?;
                        m
                    };
                    if fin {
                        Ok(Some(msg.complete()?))
                    } else {
                        self.incomplete = Some(msg);
                        Ok(None)
                    }
                }
                Data::Reserved(i) => Err(Error::Protocol(
                    format!("Unknown data frame type {}", i).into(),
                )),
            },
            _ => unreachable!(),
        }
    }
}
