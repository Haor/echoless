pub(crate) fn copy_or_zero(src: &[f32], dst: &mut [f32]) {
    let n = dst.len().min(src.len());
    dst[..n].copy_from_slice(&src[..n]);
    dst[n..].fill(0.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_or_zero_copies_exact_length() {
        let mut dst = [0.0; 3];

        copy_or_zero(&[1.0, 2.0, 3.0], &mut dst);

        assert_eq!(dst, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn copy_or_zero_truncates_long_source() {
        let mut dst = [0.0; 2];

        copy_or_zero(&[1.0, 2.0, 3.0], &mut dst);

        assert_eq!(dst, [1.0, 2.0]);
    }

    #[test]
    fn copy_or_zero_fills_long_destination() {
        let mut dst = [9.0; 4];

        copy_or_zero(&[1.0, 2.0], &mut dst);

        assert_eq!(dst, [1.0, 2.0, 0.0, 0.0]);
    }

    #[test]
    fn copy_or_zero_silences_empty_source() {
        let mut dst = [9.0; 3];

        copy_or_zero(&[], &mut dst);

        assert_eq!(dst, [0.0, 0.0, 0.0]);
    }
}
