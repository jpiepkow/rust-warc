use rust_warc::WarcReader;

use std::io;

fn main() {
    let stdin = io::stdin();
    let handle = stdin.lock();
    let warc = WarcReader::new(handle);

    let mut response_counter = 0;
    let mut response_size = 0;

    for item in warc {
        let record = item.unwrap(); // could be IO/malformed error

        // header names are case insensitive
        if record.header.get(&"WARC-Type".into()) == Some(&"response".into()) {
            response_counter += 1;
            response_size += record.content.len();
        }
    }

    println!("response records: {}", response_counter);
    println!("response size: {} MiB", response_size >> 20);
}
