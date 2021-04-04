use std::fmt;

/// Indicates the specific type/cause of a capacity error.
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum CapacityError {
    /// Too many headers provided (see [`httparse::Error::TooManyHeaders`]).
    TooManyHeaders,
    /// Received header is too long.
    HeaderTooLong,
    /// Message is bigger than the maximum allowed size.
    MessageTooLong {
        /// The size of the message.
        size: usize,
        /// The maximum allowed message size.
        max_size: usize,
    },
    /// TCP buffer is full.
    TcpBufferFull,
}

impl fmt::Debug for CapacityError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::TooManyHeaders => write!(f, "Too many headers"),
            Self::HeaderTooLong => write!(f, "Header too long"),
            Self::MessageTooLong { size, max_size } => {
                write!(f, "Message too long: {} > {}", size, max_size)
            }
            Self::TcpBufferFull => write!(f, "Incoming TCP buffer is full"),
        }
    }
}

impl fmt::Display for CapacityError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for CapacityError {}
