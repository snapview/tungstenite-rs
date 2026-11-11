/// Generate a random frame mask.
#[inline]
pub fn generate_mask() -> [u8; 4] {
    // `rand::random::<[u8; 4]>()` would draw a whole `u32` for each byte.
    rand::random::<u32>().to_ne_bytes()
}

/// Mask/unmask a frame.
#[inline]
pub fn apply_mask(buf: &mut [u8], mask: [u8; 4]) {
    apply_mask_fast32(buf, mask);
}

/// A safe unoptimized mask application.
#[inline]
fn apply_mask_fallback(buf: &mut [u8], mask: [u8; 4]) {
    for (i, byte) in buf.iter_mut().enumerate() {
        *byte ^= mask[i & 3];
    }
}

/// Faster version of `apply_mask()` which operates on 4-byte blocks.
#[inline]
pub fn apply_mask_fast32(buf: &mut [u8], mask: [u8; 4]) {
    let mask_u32 = u32::from_ne_bytes(mask);

    // Every chunk starts at a multiple of 4, so it lines up with the mask as is.
    // The compiler vectorizes this loop; unaligned loads cost nothing extra here.
    let mut words = buf.chunks_exact_mut(4);
    for word in &mut words {
        let masked = u32::from_ne_bytes([word[0], word[1], word[2], word[3]]) ^ mask_u32;
        word.copy_from_slice(&masked.to_ne_bytes());
    }
    apply_mask_fallback(words.into_remainder(), mask);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_mask() {
        let mask = [0x6d, 0xb6, 0xb2, 0x80];
        let unmasked = [
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
                let mut masked = unmasked.to_vec();
                apply_mask_fallback(&mut masked[off..], mask);

                let mut masked_fast = unmasked.to_vec();
                apply_mask_fast32(&mut masked_fast[off..], mask);

                assert_eq!(masked, masked_fast);
            }
        }
    }
}
