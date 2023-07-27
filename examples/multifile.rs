// use std::io::{Cursor, Read};

// use exact_reader::{MultiFile, File};

// fn into_file(value: Cursor<Vec<u8>>) -> File<Cursor<Vec<u8>>> {
//     let len = value.get_ref().len();
//     File {
//         file: value,
//         size: len,
//         filename: "cursor".to_string(),
//     }
// }

// fn main() {
//     let a: Cursor<Vec<u8>> = Cursor::new(vec![1u8, 2, 3]);
//     let b: Cursor<Vec<u8>> = Cursor::new(vec![4u8, 5, 6]);

//     let mut file = MultiFile::new(vec![into_file(a), into_file(b)]);
//     let mut buf = [0u8; 4];
//     file.read(&mut buf).unwrap();
//     assert_eq!(buf, [1, 2, 3, 4])
// }
