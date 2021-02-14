use std::io::Write;

/// Generate a random frame mask.
#[inline]
pub fn generate_mask() -> [u8; 4] {
    rand::random()
}

/// Write data to an output, masking the data in the process
pub fn write_masked(data: &[u8], output: &mut impl Write, mask: [u8; 4]) {
    write_mask_fast32(data, output, mask)
}

/// A safe unoptimized mask application.
#[inline]
fn write_mask_fallback(data: &[u8], output: &mut impl Write, mask: [u8; 4]) {
    for (i, byte) in data.iter().enumerate() {
        output.write(&[*byte ^ mask[i & 3]]).unwrap();
    }
}

/// Faster version of `apply_mask()` which operates on 4-byte blocks.
#[inline]
fn write_mask_fast32(data: &[u8], output: &mut impl Write, mask: [u8; 4]) {
    let mask_u32 = u32::from_ne_bytes(mask);

    let (mut prefix, words, mut suffix) = unsafe { data.align_to::<u32>() };
    write_mask_fallback(&mut prefix, output, mask);
    let head = prefix.len() & 3;
    let mask_u32 = if head > 0 {
        if cfg!(target_endian = "big") {
            mask_u32.rotate_left(8 * head as u32)
        } else {
            mask_u32.rotate_right(8 * head as u32)
        }
    } else {
        mask_u32
    };
    for word in words {
        let bytes = (*word ^ mask_u32).to_ne_bytes();
        output.write(&bytes).unwrap();
    }
    write_mask_fallback(&mut suffix, output, mask_u32.to_ne_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_mask() {
        let mask = [0x6d, 0xb6, 0xb2, 0x80];
        let unmasked = vec![
            0xf3, 0x00, 0x01, 0x02, 0x03, 0x80, 0x81, 0x82, 0xff, 0xfe, 0x00, 0x17, 0x74, 0xf9,
            0x12, 0x03,
        ];

        for data_len in 0..=unmasked.len() {
            let unmasked = &unmasked[0..data_len];
            // Check masking with different alignment.
            for off in 0..=3 {
                if unmasked.len() < off {
                    continue;
                }
                let mut masked = Vec::new();
                write_mask_fallback(&unmasked, &mut masked, mask);

                let mut masked_fast = Vec::new();
                write_mask_fast32(&unmasked, &mut masked_fast, mask);

                assert_eq!(masked, masked_fast);
            }
        }
    }
}
