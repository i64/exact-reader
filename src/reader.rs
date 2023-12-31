use std::{
    io::{Read, Seek},
    ops::RangeInclusive,
};

use crate::{
    multifile::{File, MultiFile},
    utils::calculate_seek,
    vec_deq::VecDeque,
};

/// The `ExactReader` struct simplifies reading data from a file(s).
pub struct ExactReader<R> {
    /// The inner reader for the file.
    file: R,

    /// The active range of file offsets within the buffer.
    file_offset_view: RangeInclusive<usize>,

    /// The size of the file(s).
    size: usize,

    /// The buffer used for caching data read from the file(s).
    buffer: VecDeque<u8>,
    /// The offset within the buffer
    buffer_offset: usize,

    /// Seek position to be used on `reserve`
    seeked: Option<usize>,
}

impl<R: Read + Seek> ExactReader<MultiFile<R>> {
    /// Creates a new `ExactReader` instance for reading data from multiple files.
    pub fn new_multi(file: MultiFile<R>) -> Self {
        let size = file.size();

        Self {
            file,
            size,
            buffer: VecDeque::new(),
            file_offset_view: 0..=0,
            buffer_offset: 0,
            seeked: None,
        }
    }
}

impl<R: Read + Seek> ExactReader<File<R>> {
    /// Creates a new `ExactReader` instance for reading data from a single file.
    pub fn new_single(file: File<R>) -> Self {
        let size = file.size;

        Self {
            file,
            size,
            buffer: VecDeque::new(),
            file_offset_view: 0..=0,
            buffer_offset: 0,
            seeked: None,
        }
    }
}

impl<R: Read + Seek> ExactReader<R> {
    /// The total size of the file(s) in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Calculates the physical index within the file(s) from the current buffer offset.
    #[inline]
    fn physical_idx(&self) -> usize {
        self.file_offset_view.start() + self.buffer_offset
    }

    /// Reads the given range from the inner file(s).
    fn _read(&mut self, buf: &mut Vec<u8>, read_size: usize, head: usize, tail: usize) {
        let _ = self.file.by_ref().take(read_size as u64).read_to_end(buf);
        self.file_offset_view = head..=tail;
    }

    /// Reserves and caches space in the buffer for future reads
    pub fn reserve(&mut self, reserve_size: usize) {
        let real_head = self.file_offset_view.start();

        if let Some(seek_head) = self.seeked.take() {
            let seek_tail = seek_head + reserve_size;

            if self.file_offset_view.contains(&seek_head) {
                self.buffer_offset = seek_head - real_head;
            } else if self.file_offset_view.contains(&seek_tail) {
                let read_size = self.file_offset_view.start() - seek_head;
                let mut buf: Vec<u8> = Vec::with_capacity(read_size); // TODO: make it zero copy

                self._read(&mut buf, read_size, seek_head, seek_tail);
                self.buffer_offset = 0;

                self.buffer.extend_front(buf.as_slice());
                return;
            }
            let mut buf: Vec<u8> = Vec::with_capacity(reserve_size); // TODO: make it zero copy
            self._read(&mut buf, reserve_size, seek_head, seek_tail);

            self.buffer_offset = 0;

            self.buffer.clear();
            self.buffer.extend_back(buf.as_slice());

            return;
        }

        if self.buffer.len() >= self.buffer_offset + reserve_size {
            return;
        }

        let mut buf: Vec<u8> = Vec::with_capacity(reserve_size); // TODO: make it zero copy
        let tail = self.file_offset_view.start() + self.buffer.len() + buf.len();
        self._read(&mut buf, reserve_size, *self.file_offset_view.start(), tail);

        self.buffer.extend_back(buf.as_mut_slice());
    }
}

impl<R: Read + Seek> Read for ExactReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // TODO: read when size > file.size
        let size = buf.len();
        self.reserve(size);

        let (head, tail) = self.buffer.as_slices();
        let head_len = head.len();
        let adjusted_head_len = head_len.saturating_sub(self.buffer_offset);
        let tail_offset = self.buffer_offset.saturating_sub(head_len);

        if adjusted_head_len == 0 {
            // The buffer_offset is in the tail slice
            buf.copy_from_slice(&tail[tail_offset..tail_offset + size]);
        } else if adjusted_head_len >= size {
            // The data is entirely in the head slice
            buf.copy_from_slice(&head[self.buffer_offset..self.buffer_offset + size]);
        } else {
            // Data spans both head and tail slices
            buf[..adjusted_head_len].copy_from_slice(&head[self.buffer_offset..]);
            buf[adjusted_head_len..]
                .copy_from_slice(&tail[tail_offset..tail_offset + size - adjusted_head_len]);
        }
        self.buffer_offset += size;

        Ok(size)
    }
}

impl<R: Read + Seek> Seek for ExactReader<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let calculated_seek = calculate_seek(self.size, self.physical_idx(), pos)? as usize;
        if self.file_offset_view.contains(&calculated_seek) {
            self.buffer_offset = calculated_seek - self.file_offset_view.start();
            return Ok(calculated_seek as u64);
        }

        let result = self.file.seek(pos)?;
        self.seeked = Some(result as usize);

        Ok(result)
    }

    fn stream_position(&mut self) -> std::io::Result<u64> {
        Ok(self.physical_idx() as u64)
    }
}
