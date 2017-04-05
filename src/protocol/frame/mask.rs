use std::mem::transmute;
use rand;

/// Generate a random frame mask.
#[inline]
pub fn generate_mask() -> [u8; 4] {
    rand::random()
}

/// Mask/unmask a frame.
#[inline]
pub fn apply_mask(buf: &mut [u8], mask: &[u8; 4]) {
    // Assume that the memory is 32-bytes aligned.
    // FIXME: this assumption is not correct.
    unsafe { apply_mask_aligned32(buf, mask) }
}

/// A safe unoptimized mask application.
#[inline]
fn apply_mask_fallback(buf: &mut [u8], mask: &[u8; 4]) {
    for (i, byte) in buf.iter_mut().enumerate() {
        *byte ^= mask[i & 3];
    }
}

/// Faster version of `apply_mask()` which operates on 4-byte blocks.
///
/// Safety: `buf` must be at least 4-bytes aligned.
#[inline]
unsafe fn apply_mask_aligned32(buf: &mut [u8], mask: &[u8; 4]) {
    debug_assert_eq!(buf.as_ptr() as usize % 4, 0);

    let mask_u32 = transmute(*mask);

    let mut ptr = buf.as_mut_ptr() as *mut u32;
    for _ in 0..(buf.len() / 4) {
        *ptr ^= mask_u32;
        ptr = ptr.offset(1);
    }

    // Possible last block with less than 4 bytes.
    let last_block_start = buf.len() & !3;
    let last_block = &mut buf[last_block_start..];
    apply_mask_fallback(last_block, mask);
}

#[cfg(test)]
mod tests {

    use super::{apply_mask_fallback, apply_mask_aligned32};

    #[test]
    fn test_apply_mask() {
        let mask = [
            0x6d, 0xb6, 0xb2, 0x80,
        ];
        let unmasked = vec![
            0xf3, 0x00, 0x01, 0x02, 0x03, 0x80, 0x81, 0x82, 0xff, 0xfe, 0x00,
        ];

        let mut masked = unmasked.clone();
        apply_mask_fallback(&mut masked, &mask);

        let mut masked_aligned = unmasked.clone();
        unsafe { apply_mask_aligned32(&mut masked_aligned, &mask) };

        assert_eq!(masked, masked_aligned);
    }

}

