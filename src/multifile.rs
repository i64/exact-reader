use std::io::{Read, Seek};

use crate::utils::calculate_seek;

/// The `File` struct represents an individual file within the multi-file context.
pub struct File<R> {
    /// The inner reader for the file.
    pub file: R,
    /// The size of the file in bytes.
    pub size: usize,
    /// The name of the file.
    pub filename: String,
}

impl<R: Read> Read for File<R> {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl<R: Seek> Seek for File<R> {
    #[inline]
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

/// The `MultiFile` struct combines multiple files into a unified stream,
/// allowing sequential reading as if all files are concatenated.
pub struct MultiFile<R> {
    /// The list of files
    files: Vec<File<R>>,

    /// The cumulative offset to the current file within the multi-file context.
    /// (without the in-file offset)
    cumul_offset: usize,
    /// The offset within the current file.
    infile_offset: usize,

    /// The total size of the combined multi-file stream.
    total_len: usize,
    /// The index of the current file being read from.
    current_file_idx: usize,
}

impl<R> MultiFile<R> {
    /// Creates a new `MultiFile` instance with the provided list of files.
    pub fn new(files: Vec<File<R>>) -> Self {
        let total_len = files.iter().map(|f| f.size).sum();
        Self {
            current_file_idx: 0,
            infile_offset: 0,
            cumul_offset: 0,
            files,
            total_len,
        }
    }

    /// Converts the given position within the combined multi-file stream
    /// to the index of the corresponding file within the `files`.
    #[inline]
    fn needle_to_file(&self, needle: usize) -> Option<usize> {
        if needle > self.total_len {
            return None;
        }

        if self.cumul_offset == needle {
            return Some(self.current_file_idx);
        } else if self.cumul_offset > needle {
            let mut res = 0;
            for (idx, file) in self.files.iter().enumerate().take(self.current_file_idx) {
                if res + file.size >= needle {
                    return Some(idx);
                }
                res += file.size;
            }
        } else {
            let mut res = self.cumul_offset;
            for (idx, file) in self.files.iter().enumerate().skip(self.current_file_idx) {
                if res + file.size > needle {
                    return Some(idx);
                }
                res += file.size;
            }
        }

        unreachable!()
    }

    /// Calculates the physical offset within the combined multi-file stream.
    #[inline]
    fn physical_offset(&self) -> usize {
        self.cumul_offset + self.infile_offset
    }

    /// The total size of the multi-file stream in bytes.
    pub fn size(&self) -> usize {
        self.total_len
    }
}

impl<R: Read> Read for MultiFile<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let tail_idx;
        let mut infile = 0;

        let expected = buf.len();
        let mut taken = 0;

        'find: {
            for (idx, file) in self.files[self.current_file_idx..].iter_mut().enumerate() {
                infile = file.read(&mut buf[taken..])?;
                taken += infile;
                if taken == expected {
                    tail_idx = self.current_file_idx + idx;
                    break 'find;
                }
            }
            tail_idx = self.files.len() - 1;
        }
        let _cumul_offset: usize = self.files[self.current_file_idx..tail_idx]
            .iter()
            .map(|f| f.size)
            .sum();

        self.cumul_offset += _cumul_offset;
        self.current_file_idx = tail_idx;
        self.infile_offset = infile;

        Ok(taken)
    }
}

impl<R: Read + Seek> Seek for MultiFile<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let calculated_seek = calculate_seek(self.total_len, self.physical_offset(), pos)? as usize;
        let calculated_idx = self.needle_to_file(calculated_seek).unwrap();

        let new_cum = self.files[..calculated_idx]
            .iter()
            .map(|f| f.size)
            .sum::<usize>();

        let seek_to = calculated_seek - new_cum;

        match calculated_idx.cmp(&self.current_file_idx) {
            std::cmp::Ordering::Greater => {
                for file in self.files[..calculated_idx].iter_mut() {
                    let _ = file.seek(std::io::SeekFrom::End(0))?;
                }
            }
            std::cmp::Ordering::Less => {
                for file in self.files[calculated_idx + 1..=self.current_file_idx].iter_mut() {
                    let _ = file.seek(std::io::SeekFrom::Start(0))?;
                }
            }
            std::cmp::Ordering::Equal => {}
        }

        let res =
            self.files[calculated_idx].seek(std::io::SeekFrom::Start(seek_to as u64))? as usize;

        self.current_file_idx = calculated_idx;
        self.cumul_offset = new_cum;
        self.infile_offset = res;

        Ok((new_cum + res) as u64)
    }

    fn stream_position(&mut self) -> std::io::Result<u64> {
        Ok(self.physical_offset() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    impl From<Cursor<Vec<u8>>> for File<Cursor<Vec<u8>>> {
        fn from(value: Cursor<Vec<u8>>) -> Self {
            let len = value.get_ref().len();
            Self {
                file: value,
                size: len,
                filename: "cursor".to_string(),
            }
        }
    }

    fn new_file() -> MultiFile<Cursor<Vec<u8>>> {
        let a = Cursor::new(vec![1u8, 2, 3]);
        let b = Cursor::new(vec![4u8, 5, 6]);

        MultiFile::new(vec![a.into(), b.into()])
    }
    #[test]
    fn test_read1() {
        let a = Cursor::new(vec![1u8, 2, 3]);
        let mut file = MultiFile::new(vec![a.into()]);

        {
            let mut buf = [0u8; 2];
            let _ = file.seek(std::io::SeekFrom::End(-2));

            let _ = file.read(&mut buf).unwrap();
            assert_eq!(buf, [2, 3])
        }
    }
    #[test]
    fn test_read2() {
        let mut file = new_file();

        {
            let mut buf = [0u8; 3];
            file.read(&mut buf).unwrap();
            assert_eq!(buf, [1, 2, 3])
        }

        {
            let mut buf = [0u8; 1];
            file.read(&mut buf).unwrap();
            assert_eq!(buf, [4])
        }

        {
            let mut buf = [0u8; 5];
            file.read(&mut buf).unwrap();
            assert_eq!(buf, [5, 6, 0, 0, 0])
        }
    }

    #[test]
    fn test_seek() {
        let mut file = new_file();

        {
            let mut buf = [0u8; 1];

            let _ = file.seek(std::io::SeekFrom::Start(3));

            let _ = file.read(&mut buf).unwrap();
            assert_eq!(buf, [4])
        }

        {
            let mut buf = [0u8; 1];

            let _ = file.seek(std::io::SeekFrom::Current(0));

            let _ = file.read(&mut buf).unwrap();
            assert_eq!(buf, [5])
        }
        {
            let mut buf = [0u8; 2];

            let _ = file.seek(std::io::SeekFrom::Current(-1));

            let _ = file.read(&mut buf).unwrap();
            assert_eq!(buf, [4, 5])
        }

        {
            let mut buf = [0u8; 5];

            let _ = file.seek(std::io::SeekFrom::Start(0));

            let _ = file.read(&mut buf).unwrap();
            assert_eq!(buf, [1, 2, 3, 4, 5])
        }
    }

    #[test]
    fn test_seek2() {
        let mut file = new_file();

        {
            let mut buf = [0u8; 1];

            let _ = file.seek(std::io::SeekFrom::Start(0));

            let _ = file.read(&mut buf).unwrap();
            assert_eq!(buf, [1])
        }

        {
            let mut buf = [0u8; 2];

            let _ = file.seek(std::io::SeekFrom::Start(0));

            let _ = file.read(&mut buf).unwrap();
            assert_eq!(buf, [1, 2])
        }
    }
}
