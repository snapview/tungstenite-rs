use bytes::{Buf, BytesMut};
use std::{
    ops::{Deref, DerefMut},
    ptr,
};

/// [`BytesMut`] wrapper that tracks initialization state of its spare capacity.
///
/// Supports safe & efficient repeated calls to [`Self::resize`] + [`Self::truncate`].
///
/// This optimisation is useful for [`std::io::Read`] to safely provide spare
/// capacity as an initialized slice.
///
/// Related, may be obsoleted by: <https://github.com/rust-lang/rust/issues/78485>
#[derive(Debug, Default)]
pub struct InitAwareBuf {
    bytes: BytesMut,
    /// Capacity that has been initialized.
    init_cap: usize,
}

impl InitAwareBuf {
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self { bytes: BytesMut::with_capacity(capacity), init_cap: 0 }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.bytes.capacity()
    }

    #[inline]
    pub fn split_to(&mut self, at: usize) -> BytesMut {
        let split = self.bytes.split_to(at);
        self.init_cap -= at;
        split
    }

    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        // Increasing capacity doesn't change `init_cap`
        self.bytes.reserve(additional);
    }

    /// Sets the length of the buffer to `len`. If above the current
    /// initialized capacity any uninitialized bytes will be zeroed.
    ///
    /// This is more efficient that [`BytesMut::resize`] as spare capacity
    /// is only initialized **once** past the initialized_capacity. This
    /// allow the method to be efficiently called after truncating.
    ///
    /// # Panics
    /// Panics if `len > capacity`.
    #[inline]
    pub fn resize(&mut self, len: usize) {
        if len <= self.init_cap {
            // SAFETY: init_cap tracks initialised bytes.
            unsafe {
                self.bytes.set_len(len);
            }
        } else {
            assert!(len <= self.capacity());
            let cur_len = self.bytes.len();
            let spare = self.bytes.spare_capacity_mut();
            let already_init = self.init_cap - cur_len;
            let zeroes = len - self.init_cap;
            debug_assert!(already_init + zeroes <= spare.len());
            unsafe {
                // SAFETY: spare capacity is sufficient for `zeroes` extra bytes
                ptr::write_bytes(spare[already_init..].as_mut_ptr().cast::<u8>(), 0, zeroes);
                // SAFETY: len has been initialized
                self.bytes.set_len(len);
            }
            self.init_cap = len;
        }
    }

    #[inline]
    pub fn truncate(&mut self, len: usize) {
        // truncating doesn't change `init_cap`
        self.bytes.truncate(len);
    }

    #[inline]
    pub fn advance(&mut self, cnt: usize) {
        self.bytes.advance(cnt);
        self.init_cap -= cnt;
    }
}

impl From<BytesMut> for InitAwareBuf {
    #[inline]
    fn from(bytes: BytesMut) -> Self {
        let init_cap = bytes.len();
        Self { bytes, init_cap }
    }
}

impl From<InitAwareBuf> for BytesMut {
    #[inline]
    fn from(value: InitAwareBuf) -> Self {
        value.bytes
    }
}

impl AsRef<[u8]> for InitAwareBuf {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl Deref for InitAwareBuf {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}

impl AsMut<[u8]> for InitAwareBuf {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}

impl DerefMut for InitAwareBuf {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn reserve_resize_truncate() {
        let mut buf = InitAwareBuf::default();
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.init_cap, 0);
        assert_eq!(buf.capacity(), 0);

        buf.reserve(64);
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.init_cap, 0);
        let new_capacity = buf.capacity();
        assert!(new_capacity >= 64);

        buf.resize(10);
        assert_eq!(buf.len(), 10);
        assert_eq!(buf.init_cap, 10);
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
        assert_eq!(buf.init_cap, 10);
        assert_eq!(buf.capacity(), new_capacity);
        assert_eq!(&*buf, &[8; 3]);

        // resizing should need do nothing now since this has already been initialized once
        buf.resize(10);
        assert_eq!(buf.len(), 10);
        assert_eq!(buf.init_cap, 10);
        assert_eq!(buf.capacity(), new_capacity);
        assert_eq!(&*buf, &[8, 8, 8, 44, 44, 44, 44, 44, 44, 44]);

        buf.truncate(3);
        assert_eq!(&*buf, &[8; 3]);

        // resizing should only init to zero the 3 bytes that hadn't previously been
        buf.resize(13);
        assert_eq!(buf.len(), 13);
        assert_eq!(buf.init_cap, 13);
        assert_eq!(buf.capacity(), new_capacity);
        assert_eq!(&*buf, &[8, 8, 8, 44, 44, 44, 44, 44, 44, 44, 0, 0, 0]);
    }

    #[test]
    fn advance() {
        let mut buf = InitAwareBuf::from(BytesMut::from(&[0, 1, 2, 3, 4][..]));
        assert_eq!(buf.len(), 5);
        assert_eq!(buf.init_cap, 5);

        buf.advance(2);
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.init_cap, 3);
        assert_eq!(&*buf, &[2, 3, 4]);
    }

    #[test]
    fn split_to() {
        let mut buf = InitAwareBuf::from(BytesMut::from(&[0, 1, 2, 3, 4][..]));
        assert_eq!(buf.len(), 5);
        assert_eq!(buf.init_cap, 5);

        let split = buf.split_to(2);
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.init_cap, 3);
        assert_eq!(&*buf, &[2, 3, 4]);
        assert_eq!(&*split, &[0, 1]);
    }
}
