use std::{fmt::Display, mem, string::FromUtf8Error};

use bytes::Bytes;
use core::str;

/// Utf8 payload.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct Utf8Payload(Payload);

impl Utf8Payload {
    #[inline]
    pub const fn from_static(str: &'static str) -> Self {
        Self(Payload::Shared(Bytes::from_static(str.as_bytes())))
    }

    /// Returns a slice of the payload.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        // safety: is valid uft8
        unsafe { str::from_utf8_unchecked(self.as_slice()) }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl TryFrom<Payload> for Utf8Payload {
    type Error = str::Utf8Error;

    #[inline]
    fn try_from(payload: Payload) -> Result<Self, Self::Error> {
        str::from_utf8(payload.as_slice())?;
        Ok(Self(payload))
    }
}

impl From<String> for Utf8Payload {
    #[inline]
    fn from(s: String) -> Self {
        Self(s.into())
    }
}

impl From<&str> for Utf8Payload {
    #[inline]
    fn from(s: &str) -> Self {
        Self(Payload::Owned(s.as_bytes().to_vec()))
    }
}

impl From<&String> for Utf8Payload {
    #[inline]
    fn from(s: &String) -> Self {
        s.as_str().into()
    }
}

impl From<Utf8Payload> for Payload {
    #[inline]
    fn from(Utf8Payload(payload): Utf8Payload) -> Self {
        payload
    }
}

impl Display for Utf8Payload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A payload of a WebSocket frame.
#[derive(Debug, Clone)]
pub enum Payload {
    /// Owned data with unique ownership.
    Owned(Vec<u8>),
    /// Shared data with shared ownership.
    Shared(Bytes),
}

impl Payload {
    #[inline]
    pub const fn from_static(bytes: &'static [u8]) -> Self {
        Self::Shared(Bytes::from_static(bytes))
    }

    #[inline]
    pub fn from_owner<T>(owner: T) -> Self
    where
        T: AsRef<[u8]> + Send + 'static,
    {
        Self::Shared(Bytes::from_owner(owner))
    }

    /// Returns a slice of the payload.
    #[inline]
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
    #[inline]
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
    #[inline]
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Consumes the payload and returns the underlying data as a vector.
    #[inline]
    pub fn into_data(self) -> Vec<u8> {
        match self {
            Payload::Owned(v) => v,
            Payload::Shared(v) => v.into(),
        }
    }

    /// Consumes the payload and returns the underlying data as a string.
    #[inline]
    pub fn into_text(self) -> Result<String, FromUtf8Error> {
        match self {
            Payload::Owned(v) => Ok(String::from_utf8(v)?),
            Payload::Shared(v) => Ok(String::from_utf8(v.into())?),
        }
    }
}

impl Default for Payload {
    #[inline]
    fn default() -> Self {
        Self::Owned(Vec::new())
    }
}

impl From<Vec<u8>> for Payload {
    #[inline]
    fn from(v: Vec<u8>) -> Self {
        Payload::Owned(v)
    }
}

impl From<String> for Payload {
    #[inline]
    fn from(v: String) -> Self {
        Payload::Owned(v.into())
    }
}

impl From<Bytes> for Payload {
    #[inline]
    fn from(v: Bytes) -> Self {
        Payload::Shared(v)
    }
}

impl From<&'static [u8]> for Payload {
    #[inline]
    fn from(v: &'static [u8]) -> Self {
        Self::from_static(v)
    }
}

impl From<&'static str> for Payload {
    #[inline]
    fn from(v: &'static str) -> Self {
        Self::from_static(v.as_bytes())
    }
}

impl PartialEq<Payload> for Payload {
    #[inline]
    fn eq(&self, other: &Payload) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for Payload {}

impl PartialEq<[u8]> for Payload {
    #[inline]
    fn eq(&self, other: &[u8]) -> bool {
        self.as_slice() == other
    }
}

impl<const N: usize> PartialEq<&[u8; N]> for Payload {
    #[inline]
    fn eq(&self, other: &&[u8; N]) -> bool {
        self.as_slice() == *other
    }
}
