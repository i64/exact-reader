use std::io::Cursor;

use exact_reader::{ExactReader, File};

fn into_file(value: Cursor<Vec<u8>>) -> File<Cursor<Vec<u8>>> {
    let len = value.get_ref().len();
    File {
        file: value,
        size: len,
        filename: "cursor".to_string(),
    }
}

fn main() {
    let cursor: Cursor<Vec<u8>> = Cursor::new((0u8..255).collect());
    let mut reader = ExactReader::new_single(into_file(cursor));

    reader.reserve(26);
    // read later
}
