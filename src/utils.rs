/// Calculates the new seek position in the file based on the current offset 
pub fn calculate_seek(
    size: usize,
    current_offset: usize,
    pos: std::io::SeekFrom,
) -> std::io::Result<u64> {
    let (base_pos, offset) = match pos {
        std::io::SeekFrom::Start(o) => (0, o as i64),
        std::io::SeekFrom::End(o) => (size as i64, o),
        std::io::SeekFrom::Current(o) => (current_offset as i64, o),
    };

    let new_pos = base_pos + offset;

    if new_pos.is_negative() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }

    Ok(new_pos as u64)
}
