use crate::protocol::frame::coding::Data;
use std::fmt;

/// Indicates the specific type/cause of a protocol error.
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum ProtocolError {
    /// Use of the wrong HTTP method (the WebSocket protocol requires the GET method be used).
    WrongHttpMethod,
    /// Wrong HTTP version used (the WebSocket protocol requires version 1.1 or higher).
    WrongHttpVersion,
    /// Missing `Connection: upgrade` HTTP header.
    MissingConnectionUpgradeHeader,
    /// Missing `Upgrade: websocket` HTTP header.
    MissingUpgradeWebSocketHeader,
    /// Missing `Sec-WebSocket-Version: 13` HTTP header.
    MissingSecWebSocketVersionHeader,
    /// Missing `Sec-WebSocket-Key` HTTP header.
    MissingSecWebSocketKey,
    /// The `Sec-WebSocket-Accept` header is either not present or does not specify the correct key value.
    SecWebSocketAcceptKeyMismatch,
    /// Garbage data encountered after client request.
    JunkAfterRequest,
    /// Custom responses must be unsuccessful.
    CustomResponseSuccessful,
    /// No more data while still performing handshake.
    HandshakeIncomplete,
    /// Wrapper around a [`httparse::Error`] value.
    HttparseError(httparse::Error),
    /// Not allowed to send after having sent a closing frame.
    SendAfterClosing,
    /// Remote sent data after sending a closing frame.
    ReceivedAfterClosing,
    /// Reserved bits in frame header are non-zero.
    NonZeroReservedBits,
    /// The server must close the connection when an unmasked frame is received.
    UnmaskedFrameFromClient,
    /// The client must close the connection when a masked frame is received.
    MaskedFrameFromServer,
    /// Control frames must not be fragmented.
    FragmentedControlFrame,
    /// Control frames must have a payload of 125 bytes or less.
    ControlFrameTooBig,
    /// Type of control frame not recognised.
    UnknownControlFrameType(u8),
    /// Type of data frame not recognised.
    UnknownDataFrameType(u8),
    /// Received a continue frame despite there being nothing to continue.
    UnexpectedContinueFrame,
    /// Received data while waiting for more fragments.
    ExpectedFragment(Data),
    /// Connection closed without performing the closing handshake.
    ResetWithoutClosingHandshake,
    /// Encountered an invalid opcode.
    InvalidOpcode(u8),
    /// The payload for the closing frame is invalid.
    InvalidCloseSequence,
}

impl fmt::Debug for ProtocolError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::WrongHttpMethod => {
                write!(f, "Unsupported HTTP method used - only GET is allowed")
            }
            Self::WrongHttpVersion => write!(f, "HTTP version must be 1.1 or higher"),
            Self::MissingConnectionUpgradeHeader => write!(f, "No \"Connection: upgrade\" header"),
            Self::MissingUpgradeWebSocketHeader => write!(f, "No \"Upgrade: websocket\" header"),
            Self::MissingSecWebSocketVersionHeader => {
                write!(f, "No \"Sec-WebSocket-Version: 13\" header")
            }
            Self::MissingSecWebSocketKey => write!(f, "No \"Sec-WebSocket-Key\" header"),
            Self::SecWebSocketAcceptKeyMismatch => {
                write!(f, "Key mismatch in \"Sec-WebSocket-Accept\" header")
            }
            Self::JunkAfterRequest => write!(f, "Junk after client request"),
            Self::CustomResponseSuccessful => write!(f, "Custom response must not be successful"),
            Self::HandshakeIncomplete => write!(f, "Handshake not finished"),
            Self::HttparseError(elem) => write!(f, "httparse error: {}", elem),
            Self::SendAfterClosing => write!(f, "Sending after closing is not allowed"),
            Self::ReceivedAfterClosing => write!(f, "Remote sent after having closed"),
            Self::NonZeroReservedBits => write!(f, "Reserved bits are non-zero"),
            Self::UnmaskedFrameFromClient => write!(f, "Received an unmasked frame from client"),
            Self::MaskedFrameFromServer => write!(f, "Received a masked frame from server"),
            Self::FragmentedControlFrame => write!(f, "Fragmented control frame"),
            Self::ControlFrameTooBig => {
                write!(f, "Control frame too big (payload must be 125 bytes or less)")
            }
            Self::UnknownControlFrameType(elem) => {
                write!(f, "Unknown control frame type: {}", elem)
            }
            Self::UnknownDataFrameType(elem) => write!(f, "Unknown data frame type: {}", elem),
            Self::UnexpectedContinueFrame => write!(f, "Continue frame but nothing to continue"),
            Self::ExpectedFragment(elem) => {
                write!(f, "While waiting for more fragments received: {}", elem)
            }
            Self::ResetWithoutClosingHandshake => {
                write!(f, "Connection reset without closing handshake")
            }
            Self::InvalidOpcode(elem) => write!(f, "Encountered invalid opcode: {}", elem),
            Self::InvalidCloseSequence => write!(f, "Invalid close sequence"),
        }
    }
}

impl fmt::Display for ProtocolError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for ProtocolError {}
