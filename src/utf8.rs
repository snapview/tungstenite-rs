use std::{cmp, error::Error, fmt, str};

#[derive(Debug, Copy, Clone)]
pub(crate) enum DecodeError<'a> {
    /// In lossy decoding insert `valid_prefix`, then `"\u{FFFD}"`,
    /// then call `decode()` again with `remaining_input`.
    Invalid { valid_prefix: &'a str, invalid_sequence: &'a [u8], remaining_input: &'a [u8] },

    /// Call the `incomplete_suffix.try_complete` method with more input when available.
    /// If no more input is available, this is an invalid byte sequence.
    Incomplete { valid_prefix: &'a str, incomplete_suffix: Incomplete },
}

impl<'a> fmt::Display for DecodeError<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            DecodeError::Invalid { valid_prefix, invalid_sequence, remaining_input } => write!(
                f,
                "found invalid byte sequence {invalid_sequence:02x?} after \
                 {valid_byte_count} valid bytes, followed by {unprocessed_byte_count} more \
                 unprocessed bytes",
                invalid_sequence = invalid_sequence,
                valid_byte_count = valid_prefix.len(),
                unprocessed_byte_count = remaining_input.len()
            ),
            DecodeError::Incomplete { valid_prefix, incomplete_suffix } => write!(
                f,
                "found incomplete byte sequence {incomplete_suffix:02x?} after \
                 {valid_byte_count} bytes",
                incomplete_suffix = incomplete_suffix,
                valid_byte_count = valid_prefix.len()
            ),
        }
    }
}

impl<'a> Error for DecodeError<'a> {}

#[derive(Debug, Copy, Clone)]
pub(crate) struct Incomplete {
    pub(crate) buffer: [u8; 4],
    pub(crate) buffer_len: u8,
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct Completed<'buffer, 'input> {
    pub(crate) result: Result<&'buffer str, &'buffer [u8]>,
    pub(crate) remaining_input: &'input [u8],
}

pub(crate) fn decode(input: &'_ [u8]) -> Result<&'_ str, DecodeError<'_>> {
    // Split off a character that is cut short at the end of `input` before validating,
    // so that `str::from_utf8` hands out the valid prefix in a single pass.
    let (head, tail) = input.split_at(input.len() - truncated_char_len(input));

    let valid = match str::from_utf8(head) {
        Ok(valid) => valid,
        Err(error) => {
            // FIXME: separate function from here to guide inlining?
            let (valid, after_valid) = input.split_at(error.valid_up_to());
            // Checking the prefix again only costs time for input that is rejected anyway.
            let valid = str::from_utf8(valid).expect("valid_up_to() ends a valid prefix");
            // A sequence that runs to the end of `head` is interrupted by the lead byte of `tail`.
            let invalid_sequence_length =
                error.error_len().unwrap_or(head.len() - error.valid_up_to());
            let (invalid, rest) = after_valid.split_at(invalid_sequence_length);
            return Err(DecodeError::Invalid {
                valid_prefix: valid,
                invalid_sequence: invalid,
                remaining_input: rest,
            });
        }
    };

    if tail.is_empty() {
        return Ok(valid);
    }
    // `tail` is shorter than its lead byte says, so it is never valid on its own.
    match str::from_utf8(tail).map_err(|error| error.error_len()) {
        Err(Some(invalid_sequence_length)) => {
            let (invalid, rest) = tail.split_at(invalid_sequence_length);
            Err(DecodeError::Invalid {
                valid_prefix: valid,
                invalid_sequence: invalid,
                remaining_input: rest,
            })
        }
        Ok(_) | Err(None) => Err(DecodeError::Incomplete {
            valid_prefix: valid,
            incomplete_suffix: Incomplete::new(tail),
        }),
    }
}

/// Returns the length of the last character of `input` if its lead byte
/// announces more bytes than `input` has left, and 0 otherwise.
fn truncated_char_len(input: &[u8]) -> usize {
    for (len, &byte) in (1..).zip(input.iter().rev().take(3)) {
        // Continuation bytes look like 0b10xx_xxxx; keep looking for the lead byte.
        if byte & 0xC0 != 0x80 {
            let width = match byte {
                0xC2..=0xDF => 2,
                0xE0..=0xEF => 3,
                0xF0..=0xF4 => 4,
                _ => 1,
            };
            return if width > len { len } else { 0 };
        }
    }
    0
}

impl Incomplete {
    pub(crate) fn new(bytes: &[u8]) -> Self {
        let mut buffer = [0, 0, 0, 0];
        let len = bytes.len();
        buffer[..len].copy_from_slice(bytes);
        Incomplete { buffer, buffer_len: len as u8 }
    }

    /// * `None`: still incomplete, call `try_complete` again with more input.
    ///   If no more input is available, this is invalid byte sequence.
    /// * `Some(completed)`: We’re done with this `Incomplete`,
    ///   with either a valid chunk on invalid byte sequence in `completed.result`.
    ///   To keep decoding, pass `completed.remaining_input` to `decode()`.
    pub(crate) fn try_complete<'input>(
        &mut self,
        input: &'input [u8],
    ) -> Option<Completed<'_, 'input>> {
        let (consumed, opt_result) = self.try_complete_offsets(input);
        let result = opt_result?;
        let remaining_input = &input[consumed..];
        let result_bytes = self.take_buffer();
        let result = match result {
            // The buffer holds at most 4 bytes, so checking them again is cheap.
            Ok(()) => str::from_utf8(result_bytes).map_err(|_| result_bytes),
            Err(()) => Err(result_bytes),
        };
        Some(Completed { result, remaining_input })
    }

    fn take_buffer(&mut self) -> &[u8] {
        let len = self.buffer_len as usize;
        self.buffer_len = 0;
        &self.buffer[..len]
    }

    /// (consumed_from_input, None): not enough input
    /// (consumed_from_input, Some(Err(()))): error bytes in buffer
    /// (consumed_from_input, Some(Ok(()))): UTF-8 string in buffer
    fn try_complete_offsets(&mut self, input: &[u8]) -> (usize, Option<Result<(), ()>>) {
        let initial_buffer_len = self.buffer_len as usize;
        let copied_from_input;
        {
            let unwritten = &mut self.buffer[initial_buffer_len..];
            copied_from_input = cmp::min(unwritten.len(), input.len());
            unwritten[..copied_from_input].copy_from_slice(&input[..copied_from_input]);
        }
        let spliced = &self.buffer[..initial_buffer_len + copied_from_input];
        match str::from_utf8(spliced) {
            Ok(_) => {
                self.buffer_len = spliced.len() as u8;
                (copied_from_input, Some(Ok(())))
            }
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                if valid_up_to > 0 {
                    let consumed = valid_up_to.checked_sub(initial_buffer_len).unwrap();
                    self.buffer_len = valid_up_to as u8;
                    (consumed, Some(Ok(())))
                } else {
                    match error.error_len() {
                        Some(invalid_sequence_length) => {
                            let consumed =
                                invalid_sequence_length.checked_sub(initial_buffer_len).unwrap();
                            self.buffer_len = invalid_sequence_length as u8;
                            (consumed, Some(Err(())))
                        }
                        None => {
                            self.buffer_len = spliced.len() as u8;
                            (copied_from_input, None)
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks `decode()` against what `str::from_utf8()` reports for the whole input.
    fn check_decode(input: &[u8]) {
        let (valid_up_to, error_len) = match str::from_utf8(input) {
            Ok(_) => (input.len(), None),
            Err(error) => (error.valid_up_to(), Some(error.error_len())),
        };
        match (decode(input), error_len) {
            (Ok(valid), None) => assert_eq!(valid.as_bytes(), input),
            (
                Err(DecodeError::Invalid { valid_prefix, invalid_sequence, remaining_input }),
                Some(Some(len)),
            ) => {
                assert_eq!(valid_prefix.as_bytes(), &input[..valid_up_to], "{input:x?}");
                assert_eq!(invalid_sequence, &input[valid_up_to..][..len], "{input:x?}");
                assert_eq!(remaining_input, &input[valid_up_to + len..], "{input:x?}");
            }
            (Err(DecodeError::Incomplete { valid_prefix, incomplete_suffix }), Some(None)) => {
                assert_eq!(valid_prefix.as_bytes(), &input[..valid_up_to], "{input:x?}");
                let suffix = &incomplete_suffix.buffer[..incomplete_suffix.buffer_len as usize];
                assert_eq!(suffix, &input[valid_up_to..], "{input:x?}");
            }
            (result, expected) => panic!("{input:x?}: got {result:?}, expected {expected:?}"),
        }
    }

    #[test]
    fn decode_matches_from_utf8() {
        // Bytes at the edges of the UTF-8 ranges: ASCII, continuation, overlong,
        // surrogate and out-of-range lead bytes.
        const BYTES: &[u8] = &[
            0x00, 0x41, 0x7f, 0x80, 0x8f, 0x90, 0x9f, 0xa0, 0xbf, 0xc0, 0xc1, 0xc2, 0xdf, 0xe0,
            0xe1, 0xed, 0xee, 0xef, 0xf0, 0xf1, 0xf4, 0xf5, 0xff,
        ];
        const MAX_LEN: usize = if cfg!(miri) { 2 } else { 4 };

        let mut sequences = vec![vec![]];
        let mut last = vec![vec![]];
        for _ in 0..MAX_LEN {
            last = last
                .iter()
                .flat_map(|seq| {
                    BYTES.iter().map(move |&byte| {
                        let mut seq = seq.clone();
                        seq.push(byte);
                        seq
                    })
                })
                .collect();
            sequences.extend(last.iter().cloned());
        }

        for prefix in ["", "a", "\u{e9}", "\u{20ac}", "\u{1f600}"] {
            for sequence in &sequences {
                let mut input = prefix.as_bytes().to_vec();
                input.extend_from_slice(sequence);
                check_decode(&input);
            }
        }
    }

    #[test]
    fn try_complete_returns_str() {
        let euro = "\u{20ac}".as_bytes();
        let Err(DecodeError::Incomplete { mut incomplete_suffix, .. }) = decode(&euro[..1]) else {
            panic!("expected an incomplete suffix");
        };
        assert!(incomplete_suffix.try_complete(&euro[1..2]).is_none());
        let completed = incomplete_suffix.try_complete(&euro[2..]).unwrap();
        assert_eq!(completed.result, Ok("\u{20ac}"));
        assert_eq!(completed.remaining_input, b"");

        let Err(DecodeError::Incomplete { mut incomplete_suffix, .. }) = decode(&euro[..2]) else {
            panic!("expected an incomplete suffix");
        };
        let completed = incomplete_suffix.try_complete(b"ab").unwrap();
        assert_eq!(completed.result, Err(&euro[..2]));
        assert_eq!(completed.remaining_input, b"ab");
    }
}
