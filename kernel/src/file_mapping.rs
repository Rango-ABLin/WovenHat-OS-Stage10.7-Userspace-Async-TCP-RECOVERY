//! Bounds for the private file mapping ABIs.
pub const MAX_SIZE: usize = 64 * 1024;
pub fn mapped_size(file_size: usize, offset: usize, length: usize) -> Option<usize> {
    if length == 0
        || length > MAX_SIZE
        || !offset.is_multiple_of(4096)
        || offset.checked_add(length)? > file_size
    {
        return None;
    }
    length.checked_add(4095).map(|n| n & !4095)
}
#[cfg(test)]
mod tests {
    #[test]
    fn range_validation() {
        assert_eq!(super::mapped_size(4103, 4096, 7), Some(4096));
        assert_eq!(super::mapped_size(65536, 0, 65536), Some(65536));
        for (file, offset, length) in [
            (0, 0, 1),
            (4096, 0, 0),
            (4096, 1, 1),
            (4096, 4096, 1),
            (4096, 0, 4097),
            (70000, 0, 65537),
            (usize::MAX, usize::MAX - 4095, 4096),
        ] {
            assert_eq!(super::mapped_size(file, offset, length), None);
        }
    }
}
