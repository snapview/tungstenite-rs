use crate::extensions::WebSocketExtension;
use crate::protocol::frame::coding::Data;
use crate::protocol::frame::ExtensionHeaders;
use crate::protocol::message::{IncompleteMessage, IncompleteMessageType};
use crate::protocol::MAX_MESSAGE_SIZE;
use crate::{Error, Message};

/// An uncompressed message handler for a WebSocket.
#[derive(Debug)]
pub struct UncompressedExt {
    incomplete: Option<IncompleteMessage>,
    max_message_size: Option<usize>,
}

impl Default for UncompressedExt {
    fn default() -> Self {
        UncompressedExt {
            incomplete: None,
            max_message_size: Some(MAX_MESSAGE_SIZE),
        }
    }
}

impl UncompressedExt {
    /// Builds a new `UncompressedExt` that will permit a maximum message size of `max_message_size`
    /// or will be unbounded if `None`.
    pub fn new(max_message_size: Option<usize>) -> UncompressedExt {
        UncompressedExt {
            incomplete: None,
            max_message_size,
        }
    }
}

impl WebSocketExtension for UncompressedExt {
    fn on_receive_frame(
        &mut self,
        data_opcode: Data,
        is_final: bool,
        header: ExtensionHeaders,
        payload: Vec<u8>,
    ) -> Result<Option<Message>, crate::Error> {
        let fin = is_final;

        if header.rsv1 || header.rsv2 || header.rsv3 {
            return Err(Error::Protocol(
                "Reserved bits are non-zero and no WebSocket extensions are enabled".into(),
            ));
        }

        match data_opcode {
            Data::Continue => {
                if let Some(ref mut msg) = self.incomplete {
                    msg.extend(payload, self.max_message_size)?;
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
                    let message_type = match data_opcode {
                        Data::Text => IncompleteMessageType::Text,
                        Data::Binary => IncompleteMessageType::Binary,
                        _ => panic!("Bug: message is not text nor binary"),
                    };
                    let mut m = IncompleteMessage::new(message_type);
                    m.extend(payload, self.max_message_size)?;
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
        }
    }
}
