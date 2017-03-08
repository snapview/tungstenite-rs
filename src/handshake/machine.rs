use std::io::{Cursor, Read, Write};
use bytes::Buf;

use input_buffer::{InputBuffer, MIN_READ};
use error::{Error, Result};
use util::NonBlockingResult;

/// A generic handshake state machine.
pub struct HandshakeMachine<Stream> {
    stream: Stream,
    state: HandshakeState,
}

impl<Stream> HandshakeMachine<Stream> {
    /// Start reading data from the peer.
    pub fn start_read(stream: Stream) -> Self {
        HandshakeMachine {
            stream: stream,
            state: HandshakeState::Reading(InputBuffer::with_capacity(MIN_READ)),
        }
    }
    /// Start writing data to the peer.
    pub fn start_write<D: Into<Vec<u8>>>(stream: Stream, data: D) -> Self {
        HandshakeMachine {
            stream: stream,
            state: HandshakeState::Writing(Cursor::new(data.into())),
        }
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

impl<Stream: Read + Write> HandshakeMachine<Stream> {
    /// Perform a single handshake round.
    pub fn single_round<Obj: TryParse>(mut self) -> Result<RoundResult<Obj, Stream>> {
        Ok(match self.state {
            HandshakeState::Reading(mut buf) => {
                buf.reserve(MIN_READ, usize::max_value()) // TODO limit size
                    .map_err(|_| Error::Capacity("Header too long".into()))?;
                if let Some(_) = buf.read_from(&mut self.stream).no_block()? {
                    if let Some((size, obj)) = Obj::try_parse(Buf::bytes(&buf))? {
                        buf.advance(size);
                        RoundResult::StageFinished(StageResult::DoneReading {
                            result: obj,
                            stream: self.stream,
                            tail: buf.into_vec(),
                        })
                    } else {
                        RoundResult::Incomplete(HandshakeMachine {
                            state: HandshakeState::Reading(buf),
                            ..self
                        })
                    }
                } else {
                    RoundResult::WouldBlock(HandshakeMachine {
                        state: HandshakeState::Reading(buf),
                        ..self
                    })
                }
            }
            HandshakeState::Writing(mut buf) => {
                if let Some(size) = self.stream.write(Buf::bytes(&buf)).no_block()? {
                    buf.advance(size);
                    if buf.has_remaining() {
                        RoundResult::Incomplete(HandshakeMachine {
                            state: HandshakeState::Writing(buf),
                            ..self
                        })
                    } else {
                        RoundResult::StageFinished(StageResult::DoneWriting(self.stream))
                    }
                } else {
                    RoundResult::WouldBlock(HandshakeMachine {
                        state: HandshakeState::Writing(buf),
                        ..self
                    })
                }
            }
        })
    }
}

/// The result of the round.
pub enum RoundResult<Obj, Stream> {
    /// Round not done, I/O would block.
    WouldBlock(HandshakeMachine<Stream>),
    /// Round done, state unchanged.
    Incomplete(HandshakeMachine<Stream>),
    /// Stage complete.
    StageFinished(StageResult<Obj, Stream>),
}

/// The result of the stage.
pub enum StageResult<Obj, Stream> {
    /// Reading round finished.
    DoneReading { result: Obj, stream: Stream, tail: Vec<u8> },
    /// Writing round finished.
    DoneWriting(Stream),
}

/// The parseable object.
pub trait TryParse: Sized {
    /// Return Ok(None) if incomplete, Err on syntax error.
    fn try_parse(data: &[u8]) -> Result<Option<(usize, Self)>>;
}

/// The handshake state.
enum HandshakeState {
    /// Reading data from the peer.
    Reading(InputBuffer),
    /// Sending data to the peer.
    Writing(Cursor<Vec<u8>>),
}
