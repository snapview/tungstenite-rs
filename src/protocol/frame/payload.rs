use std::{mem, string::FromUtf8Error};

use bytes::Bytes;

/// A payload of a WebSocket frame.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Payload {
    /// Owned data with unique ownership.
    Owned(Vec<u8>),
    /// Shared data with shared ownership.
    Shared(Bytes),
}

impl Payload {
    /// Returns a slice of the payload.
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Payload::Owned(v) => v,
            Payload::Shared(v) => v,
        }
    }

    /// Returns a mutable slice of the payload.
    ///
    /// Note that this will internally allocate if the payload is shared
    /// and there are other references to the same data. No allocation
    /// would happen if the payload is owned or if there is only one
    /// `Bytes` instance referencing the data.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        match self {
            Payload::Owned(v) => &mut *v,
            Payload::Shared(v) => {
                // Using `Bytes::to_vec()` or `Vec::from(bytes.as_ref())` would mean making a copy.
                // `Bytes::into()` would not make a copy if our `Bytes` instance is the only one.
                let data = mem::take(v).into();
                *self = Payload::Owned(data);
                match self {
                    Payload::Owned(v) => v,
                    Payload::Shared(_) => unreachable!(),
                }
            }
        }
    }

    /// Returns the length of the payload.
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Consumes the payload and returns the underlying data as a vector.
    pub fn into_data(self) -> Vec<u8> {
        match self {
            Payload::Owned(v) => v,
            Payload::Shared(v) => v.into(),
        }
    }

    /// Consumes the payload and returns the underlying data as a string.
    pub fn into_text(self) -> Result<String, FromUtf8Error> {
        match self {
            Payload::Owned(v) => Ok(String::from_utf8(v)?),
            Payload::Shared(v) => Ok(String::from_utf8(v.into())?),
        }
    }
}

impl From<Vec<u8>> for Payload {
    fn from(v: Vec<u8>) -> Self {
        Payload::Owned(v)
    }
}

impl From<String> for Payload {
    fn from(v: String) -> Self {
        Payload::Owned(v.into_bytes())
    }
}

impl From<Bytes> for Payload {
    fn from(v: Bytes) -> Self {
        Payload::Shared(v)
    }
}

impl From<&'static [u8]> for Payload {
    fn from(v: &'static [u8]) -> Self {
        Payload::Shared(Bytes::from_static(v))
    }
}

impl From<&'static str> for Payload {
    fn from(v: &'static str) -> Self {
        Payload::Shared(Bytes::from_static(v.as_bytes()))
    }
}
