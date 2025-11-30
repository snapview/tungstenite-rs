use bytes::{Buf, BytesMut};
use std::ops::{Deref, DerefMut};

/// Buffer that provides fast & safe [`Self::resize`] & [`Self::truncate`] usage.
///
/// It is aware of the initialization state of its spare capacity avoiding the
/// need to zero uninitialized bytes on resizing more than once for safe usage.
///
/// This optimisation is useful for [`std::io::Read`] to safely provide spare
/// capacity as an initialized slice.
///
/// Related, may be obsoleted by: <https://github.com/rust-lang/rust/issues/78485>
#[derive(Debug, Default)]
pub struct InitAwareBuf {
    /// Backing buf length is used as capacity. This ensure this extra region
    /// is always initialized (initially with zero, but otherwise the last previously
    /// set value).
    bytes: BytesMut,
    /// Length of bytes in use (always <= bytes.len).
    len: usize,
}

impl InitAwareBuf {
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self { bytes: BytesMut::zeroed(capacity), len: 0 }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Capacity that may be resized to cheaply.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.bytes.len()
    }

    #[inline]
    pub fn split_to(&mut self, at: usize) -> BytesMut {
        assert!(at <= self.len, "split_to out of bounds: {at} <= {}", self.len);
        let split = self.bytes.split_to(at);
        self.len -= at;
        split
    }

    /// Reserve capacity for `min_additional` more bytes than the current [`Self::len`].
    ///
    /// `max_additional` sets the maximum number of additional bytes zeroed as extra
    /// capacity if available after reserving in the underlying buffer. Has no effect
    /// if `max_additional <= additional`.
    #[inline]
    pub fn reserve(&mut self, additional: usize, max_additional: usize) {
        let min_len = self.len + additional;
        let cap = self.capacity();
        if min_len > cap {
            self.bytes.reserve(min_len - cap);
            let new_cap = self.bytes.capacity().min(self.len + max_additional.max(additional));
            self.bytes.resize(new_cap, 0);
        }
    }

    /// Resizes the buffer to `new_len`.
    ///
    /// If greater the new bytes will be either initialized to zero or as
    /// they were last set to.
    #[inline]
    pub fn resize(&mut self, new_len: usize) {
        if new_len > self.capacity() {
            self.bytes.resize(new_len, 0);
        }
        self.len = new_len;
    }

    #[inline]
    pub fn truncate(&mut self, len: usize) {
        if len < self.len {
            self.len = len;
        }
    }

    #[inline]
    pub fn advance(&mut self, cnt: usize) {
        assert!(cnt <= self.len, "cannot advance past len: {cnt} <= {}", self.len);
        self.bytes.advance(cnt);
        self.len -= cnt;
    }
}

impl From<BytesMut> for InitAwareBuf {
    #[inline]
    fn from(bytes: BytesMut) -> Self {
        let len = bytes.len();
        Self { bytes, len }
    }
}

impl From<InitAwareBuf> for BytesMut {
    #[inline]
    fn from(mut zb: InitAwareBuf) -> Self {
        zb.bytes.truncate(zb.len);
        zb.bytes
    }
}

impl AsRef<[u8]> for InitAwareBuf {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Deref for InitAwareBuf {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.as_ref()
    }
}

impl AsMut<[u8]> for InitAwareBuf {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[..self.len]
    }
}

impl DerefMut for InitAwareBuf {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        self.as_mut()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn reserve_resize_truncate() {
        let mut buf = InitAwareBuf::default();
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), 0);

        buf.reserve(64, 0);
        assert_eq!(buf.len(), 0);
        let new_capacity = buf.capacity();
        assert!(new_capacity >= 64);

        buf.resize(10);
        assert_eq!(buf.len(), 10);
        assert_eq!(buf.capacity(), new_capacity);
        assert_eq!(&*buf, &[0; 10]);

        // write 3 bytes =8
        buf[0] = 8;
        buf[1] = 8;
        buf[2] = 8;
        // mark the other bytes as =44
        for i in 3..10 {
            buf[i] = 44;
        }
        buf.truncate(3);
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.capacity(), new_capacity);
        assert_eq!(&*buf, &[8; 3]);

        // resizing should need do nothing now since this has already been initialized once
        buf.resize(10);
        assert_eq!(buf.len(), 10);
        assert_eq!(buf.capacity(), new_capacity);
        assert_eq!(&*buf, &[8, 8, 8, 44, 44, 44, 44, 44, 44, 44]);

        buf.truncate(3);
        assert_eq!(&*buf, &[8; 3]);

        // resizing should only init to zero the 3 bytes that hadn't previously been
        buf.resize(13);
        assert_eq!(buf.len(), 13);
        assert_eq!(buf.capacity(), new_capacity);
        assert_eq!(&*buf, &[8, 8, 8, 44, 44, 44, 44, 44, 44, 44, 0, 0, 0]);
    }

    #[test]
    fn advance() {
        let mut buf = InitAwareBuf::from(BytesMut::from(&[0, 1, 2, 3, 4][..]));
        assert_eq!(buf.len(), 5);
        assert_eq!(buf.capacity(), 5);

        buf.advance(2);
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.capacity(), 3);
        assert_eq!(&*buf, &[2, 3, 4]);
    }

    #[test]
    fn split_to() {
        let mut buf = InitAwareBuf::from(BytesMut::from(&[0, 1, 2, 3, 4][..]));
        assert_eq!(buf.len(), 5);
        assert_eq!(buf.capacity(), 5);

        let split = buf.split_to(2);
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.capacity(), 3);
        assert_eq!(&*buf, &[2, 3, 4]);
        assert_eq!(&*split, &[0, 1]);
    }
}
