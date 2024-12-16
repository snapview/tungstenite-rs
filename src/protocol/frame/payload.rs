use bytes::{Bytes, BytesMut};
use core::str;
use std::{fmt::Display, mem};

/// Utf8 payload.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct Utf8Payload(Payload);

impl Utf8Payload {
    /// Creates from a static str.
    #[inline]
    pub const fn from_static(str: &'static str) -> Self {
        Self(Payload::Shared(Bytes::from_static(str.as_bytes())))
    }

    /// Returns a slice of the payload.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }

    /// Returns as a string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        // safety: is valid uft8
        unsafe { str::from_utf8_unchecked(self.as_slice()) }
    }

    /// Returns length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Returns true if the length is 0.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// If owned converts into [`Bytes`] internals & then clones (cheaply).
    #[inline]
    pub fn share(&mut self) -> Self {
        Self(self.0.share())
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

impl TryFrom<Bytes> for Utf8Payload {
    type Error = str::Utf8Error;

    #[inline]
    fn try_from(bytes: Bytes) -> Result<Self, Self::Error> {
        Payload::from(bytes).try_into()
    }
}

impl TryFrom<BytesMut> for Utf8Payload {
    type Error = str::Utf8Error;

    #[inline]
    fn try_from(bytes: BytesMut) -> Result<Self, Self::Error> {
        Payload::from(bytes).try_into()
    }
}

impl TryFrom<Vec<u8>> for Utf8Payload {
    type Error = str::Utf8Error;

    #[inline]
    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        Payload::from(bytes).try_into()
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
        Self(Payload::Owned(s.as_bytes().into()))
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

impl AsRef<str> for Utf8Payload {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<[u8]> for Utf8Payload {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl<T> PartialEq<T> for Utf8Payload
where
    for<'a> &'a str: PartialEq<T>,
{
    /// ```
    /// use tungstenite::protocol::frame::Utf8Payload;
    /// let payload = Utf8Payload::from_static("foo123");
    /// assert_eq!(payload, "foo123");
    /// assert_eq!(payload, "foo123".to_string());
    /// assert_eq!(payload, &"foo123".to_string());
    /// assert_eq!(payload, std::borrow::Cow::from("foo123"));
    /// ```
    #[inline]
    fn eq(&self, other: &T) -> bool {
        self.as_str() == *other
    }
}

/// A payload of a WebSocket frame.
#[derive(Debug, Clone)]
pub enum Payload {
    /// Owned data with unique ownership.
    Owned(BytesMut),
    /// Shared data with shared ownership.
    Shared(Bytes),
    /// Owned vec data.
    Vec(Vec<u8>),
}

impl Payload {
    /// Creates from static bytes.
    #[inline]
    pub const fn from_static(bytes: &'static [u8]) -> Self {
        Self::Shared(Bytes::from_static(bytes))
    }

    /// Converts into [`Bytes`] internals & then clones (cheaply).
    pub fn share(&mut self) -> Self {
        match self {
            Self::Owned(data) => {
                *self = Self::Shared(mem::take(data).freeze());
            }
            Self::Vec(data) => {
                *self = Self::Shared(Bytes::from(mem::take(data)));
            }
            Self::Shared(_) => {}
        }
        self.clone()
    }

    /// Returns a slice of the payload.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Payload::Owned(v) => v,
            Payload::Shared(v) => v,
            Payload::Vec(v) => v,
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
            Payload::Vec(v) => &mut *v,
            Payload::Shared(v) => {
                // Using `Bytes::to_vec()` or `Vec::from(bytes.as_ref())` would mean making a copy.
                // `Bytes::into()` would not make a copy if our `Bytes` instance is the only one.
                let data = mem::take(v).into();
                *self = Payload::Owned(data);
                match self {
                    Payload::Owned(v) => v,
                    _ => unreachable!(),
                }
            }
        }
    }

    /// Returns the length of the payload.
    #[inline]
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Returns true if the payload has a length of 0.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Consumes the payload and returns the underlying data as a string.
    #[inline]
    pub fn into_text(self) -> Result<Utf8Payload, str::Utf8Error> {
        self.try_into()
    }
}

impl Default for Payload {
    #[inline]
    fn default() -> Self {
        Self::Owned(<_>::default())
    }
}

impl From<Vec<u8>> for Payload {
    #[inline]
    fn from(v: Vec<u8>) -> Self {
        Payload::Vec(v)
    }
}

impl From<String> for Payload {
    #[inline]
    fn from(v: String) -> Self {
        v.into_bytes().into()
    }
}

impl From<Bytes> for Payload {
    #[inline]
    fn from(v: Bytes) -> Self {
        Payload::Shared(v)
    }
}

impl From<BytesMut> for Payload {
    #[inline]
    fn from(v: BytesMut) -> Self {
        Payload::Owned(v)
    }
}

impl From<&[u8]> for Payload {
    #[inline]
    fn from(v: &[u8]) -> Self {
        Self::Owned(v.into())
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

impl AsRef<[u8]> for Payload {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}
