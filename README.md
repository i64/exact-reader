# exact-read

[![Documentation](https://docs.rs/exact-reader/badge.svg)](https://docs.rs/exact-reader/)
[![crates](https://img.shields.io/crates/v/exact-reader.svg)](https://crates.io/crates/exact-reader)


The `exact-read` crate is an IO buffering library that provides reservation-based buffering for efficient data reading from files. Reservation-based buffering allows deciding buffer size hand at any time. That allows minimum I/O operation for expensive systems like networks. Additionally, the crate supports virtually-concatenated files, allowing separated files to be treated and seek/read as if they were concatenated into one continuous stream.

Since `MultiFile`, `File`, and `ExactReader` use and implement `Seek + Read`, these structs can be used separately without requiring them to be chained.

```rust
use std::io::{Cursor, Read};

use exact_reader::{MultiFile, File, ExactReader};

fn into_file(value: Cursor<Vec<u8>>) -> File<Cursor<Vec<u8>>> {
    let len = value.get_ref().len();
    File {
        file: value,
        size: len,
        filename: "cursor".to_string(),
    }
}

fn main() {
    let a: Cursor<Vec<u8>> = Cursor::new(vec![1u8, 2, 3]);
    let b: Cursor<Vec<u8>> = Cursor::new(vec![4u8, 5, 6]);

    let mut multifile = MultiFile::new(vec![into_file(a), into_file(b)]);
    let mut reader = ExactReader::new_multi(multifile);
    reader.reserve(6);

    let mut buf = [0u8; 4];
    reader.read(&mut buf).unwrap();
    
    assert_eq!(buf, [1, 2, 3, 4])
}
```