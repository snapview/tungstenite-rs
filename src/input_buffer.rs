use std::io::{Cursor, Read, Result as IoResult};
use bytes::{Buf, BufMut};

/// A FIFO buffer for reading packets from network.
pub struct InputBuffer(Cursor<Vec<u8>>);

/// The minimum read size.
pub const MIN_READ: usize = 4096;

/// Size limit error.
pub struct SizeLimit;

impl InputBuffer {
    /// Create a new empty one.
    pub fn with_capacity(capacity: usize) -> Self {
        InputBuffer(Cursor::new(Vec::with_capacity(capacity)))
    }

    /// Create a new one from partially read data.
    pub fn from_partially_read(part: Vec<u8>) -> Self {
        InputBuffer(Cursor::new(part))
    }

    /// Reserve the given amount of space.
    pub fn reserve(&mut self, space: usize, limit: usize) -> Result<(), SizeLimit>{
        let remaining = self.inp_mut().capacity() - self.inp_mut().len();
        if remaining >= space {
            // We have enough space right now.
            Ok(())
        } else {
            let pos = self.out().position() as usize;
            self.inp_mut().drain(0..pos);
            self.out_mut().set_position(0);
            let avail = self.inp_mut().capacity() - self.inp_mut().len();
            if space <= avail {
                Ok(())
            } else if self.inp_mut().capacity() + space > limit {
                Err(SizeLimit)
            } else {
                self.inp_mut().reserve(space - avail);
                Ok(())
            }
        }
    }

    /// Read data from stream into the buffer.
    pub fn read_from<S: Read>(&mut self, stream: &mut S) -> IoResult<usize> {
        let size;
        let buf = self.inp_mut();
        unsafe {
            size = stream.read(buf.bytes_mut())?;
            buf.advance_mut(size);
        }
        Ok(size)
    }

    /// Get the rest of the buffer and destroy the buffer.
    pub fn into_vec(mut self) -> Vec<u8> {
        let pos = self.out().position() as usize;
        self.inp_mut().drain(0..pos);
        self.0.into_inner()
    }

    /// The output end (to the application).
    pub fn out(&self) -> &Cursor<Vec<u8>> {
        &self.0 // the cursor itself
    }
    /// The output end (to the application).
    pub fn out_mut(&mut self) -> &mut Cursor<Vec<u8>> {
        &mut self.0 // the cursor itself
    }

    /// The input end (to the network).
    fn inp_mut(&mut self) -> &mut Vec<u8> {
        self.0.get_mut() // underlying vector
    }
}

impl Buf for InputBuffer {
    fn remaining(&self) -> usize {
        Buf::remaining(self.out())
    }
    fn bytes(&self) -> &[u8] {
        Buf::bytes(self.out())
    }
    fn advance(&mut self, size: usize) {
        Buf::advance(self.out_mut(), size)
    }
}
