//! A high performance Web Archive (WARC) file parser
//!
//! The WarcReader iterates over [WarcRecords](WarcRecord) from a [BufRead] input.
//!
//! Perfomance should be quite good, about ~500MiB/s on a single CPU core.
//!
//! ## Usage
//!
//! ```rust
//! use rust_warc::WarcReader;
//!
//! fn main() {
//!     // we're taking input from stdin here, but any BufRead will do
//!     let stdin = std::io::stdin();
//!     let handle = stdin.lock();
//!
//!     let mut warc = WarcReader::new(handle);
//!
//!     let mut response_counter = 0;
//!     for item in warc {
//!         let record = item.expect("IO/malformed error");
//!
//!         // header names are case insensitive
//!         if record.header.get(&"WARC-Type".into()) == Some(&"response".into()) {
//!             response_counter += 1;
//!         }
//!     }
//!
//!     println!("# response records: {}", response_counter);
//! }
//! ```

use std::collections::HashMap;
use std::io::BufRead;

// trim a string in place (no (re)allocations)
fn rtrim(s: &mut String) {
    s.truncate(s.trim_end().len());
}

/// Case insensitive string
///
/// ```
/// use rust_warc::CaseString;
///
/// // explicit constructor
/// let s1 = CaseString::from(String::from("HELLO!"));
///
/// // implicit conversion from String or &str
/// let s2: CaseString = "hello!".into();
///
/// assert_eq!(s1, s2);
/// ```
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct CaseString {
    inner: String,
}
impl CaseString {
    pub fn to_string(self) -> String {
        self.into()
    }
}

impl PartialEq<String> for CaseString {
    fn eq(&self, other: &String) -> bool {
        self.inner == other.to_ascii_lowercase()
    }
}

impl From<String> for CaseString {
    fn from(mut s: String) -> Self {
        s.make_ascii_lowercase();

        CaseString { inner: s }
    }
}
impl From<&str> for CaseString {
    fn from(s: &str) -> Self {
        String::from(s).into()
    }
}

impl Into<String> for CaseString {
    fn into(self) -> String {
        self.inner
    }
}

/// WARC Record
///
/// A record consists of the version string, a list of headers and the actual content (in bytes)
///
/// # Usage
/// ```rust
/// use rust_warc::WarcRecord;
///
/// /* test.warc:
/// WARC/1.1
/// WARC-Type: warcinfo
/// WARC-Date: 2006-09-19T17:20:14Z
/// WARC-Record-ID: multiline
///  uuid value
/// Content-Type: text/plain
/// Content-Length: 4
///
/// test
///
/// */
///
/// let mut data = &include_bytes!("test.warc")[..];
///
/// let item = WarcRecord::parse(&mut data).unwrap();
///
/// assert_eq!(item.version, "WARC/1.1");
///
/// // header names are case insensitive
/// assert_eq!(item.header.get(&"content-type".into()), Some(&"text/plain".into()));
/// // and may span multiple lines
/// assert_eq!(item.header.get(&"Warc-Record-ID".into()), Some(&"multiline\nuuid value".into()));
///
/// assert_eq!(item.content, "test".as_bytes());
/// ```
pub struct WarcRecord {
    /// WARC version string (WARC/1.1)
    pub version: String,
    /// Record header fields
    pub header: HashMap<CaseString, String>,
    /// Record content block
    pub content: Vec<u8>,
}

impl WarcRecord {
    pub fn parse(mut read: impl BufRead, sum: &mut usize) -> Result<Self, WarcError> {
        let mut version = String::new();
        let mut version_len = 0;
        let mut headers_len = 0;
        match read.read_line(&mut version) {
            Err(io) => {
                // println!("{:?}", 1);   
                return Err(WarcError::IO(io))
            },
            Ok(pos) => {
                *sum = *sum + pos;
                version_len = pos;
            }
        };

        if version.is_empty() {
           // println!("{:?}", 2);    
            return Err(WarcError::EOF);
        }

        rtrim(&mut version);

        if !version.starts_with("WARC/1.") {
            *sum = *sum - version_len;
            // println!("{:?}", 3);   
            return Err(WarcError::Malformed(String::from("Unknown WARC version")));
        }

        let mut header = HashMap::<CaseString, String>::with_capacity(16); // no allocations if <= 16 header fields

        let mut continuation: Option<(CaseString, String)> = None;
        loop {
            let mut line_buf = String::new();
            match read.read_line(&mut line_buf) {
                Err(io) => {
                    *sum = *sum - version_len - headers_len;
                    // println!("{:?}", 4);   
                    return Err(WarcError::IO(io))
                },
                Ok(pos) => {
                    *sum = *sum + pos;
                    headers_len += pos;
                }   
            }

            if &line_buf == "\r\n" {
                break;
            }

            rtrim(&mut line_buf);

            if line_buf.starts_with(' ') || line_buf.starts_with('\t') {
                if let Some(keyval) = &mut continuation {
                    keyval.1.push('\n');
                    keyval.1.push_str(line_buf.trim());
                } else {
                    *sum = *sum - version_len - headers_len;
                    // println!("{:?}", 5);   
                    return Err(WarcError::Malformed(String::from("Invalid header block")));
                }
            } else {
                if let Some((key, value)) = std::mem::replace(&mut continuation, None) {
                    header.insert(key, value);
                }

                if let Some(semi) = line_buf.find(':') {
                    let value = line_buf.split_off(semi + 1).trim().to_string();
                    line_buf.pop(); // eat colon
                    rtrim(&mut line_buf);

                    continuation = Some((line_buf.into(), value));
                } else {
                    *sum = *sum - version_len - headers_len;
                    // println!("{:?}", 6);   
                    return Err(WarcError::Malformed(String::from("Invalid header field")));
                }
            }
        }

        // insert leftover continuation
        if let Some((key, value)) = continuation {
            header.insert(key, value);
        }

        let content_len = header.get(&"Content-Length".into());
        if content_len.is_none() {
            *sum = *sum - version_len - headers_len;
            // println!("{:?}", 7);   
            return Err(WarcError::Malformed(String::from(
                "Content-Length is missing",
            )));
        }

        let content_len = content_len.unwrap().parse::<usize>();
        if content_len.is_err() {
            *sum = *sum - version_len - headers_len;
            // println!("{:?}", 8);   
            return Err(WarcError::Malformed(String::from(
                "Content-Length is not a number",
            )));
        }

        let content_len = content_len.unwrap();
        let mut content = vec![0; content_len];
        
        if let Err(io) = read.read_exact(&mut content) {

            *sum = *sum - version_len - headers_len;
            // println!("{:?}", 8);   
            return Err(WarcError::IO(io));
        } else {
            *sum = *sum + content_len;
        }

        let mut linefeed = [0u8; 4];
        
        if let Err(io) = read.read_exact(&mut linefeed) {
            *sum = *sum - version_len - headers_len - content_len;
            // println!("{:?}", 9);   
            return Err(WarcError::IO(io));
        } else {
            *sum = *sum + 4;
        }
        if linefeed != [13, 10, 13, 10] {
            *sum = *sum - version_len - headers_len - content_len;
            // println!("{:?}", 10);   
            return Err(WarcError::Malformed(String::from(
                "No double linefeed after record content",
            )));
        }

        let record = WarcRecord {
            version,
            header,
            content,
        };

        Ok(record)
    }
}

/// WARC Processing error
#[derive(Debug)]
pub enum WarcError {
    Malformed(String),
    IO(std::io::Error),
    EOF,
}

/// WARC reader instance
///
/// The WarcReader serves as an iterator for [WarcRecords](WarcRecord) (or [errors](WarcError))
///
/// # Usage
/// ```rust
/// use rust_warc::{WarcReader, WarcRecord, WarcError};
///
/// let data = &include_bytes!("warc.in")[..];
/// let mut warc = WarcReader::new(data);
///
/// let item: Option<Result<WarcRecord, WarcError>> = warc.next();
/// assert!(item.is_some());
///
/// // count remaining items
/// assert_eq!(warc.count(), 2);
/// ```
pub struct WarcReader<R> {
    pub read: R,
    pub sum: usize,
    valid_state: bool,
}

impl<R: BufRead> WarcReader<R> {
    /// Create a new WarcReader from a [BufRead] input
    pub fn new(read: R) -> Self {
        Self {
            read,
            sum: 0,
            valid_state: true,
        }
    }
}

impl<R: BufRead> Iterator for WarcReader<R> {
    type Item = Result<WarcRecord, WarcError>;

    fn next(&mut self) -> Option<Result<WarcRecord, WarcError>> {
        if !self.valid_state {
            return None;
        }

        match WarcRecord::parse(&mut self.read, &mut self.sum) {
            Ok(item) => Some(Ok(item)),
            Err(WarcError::EOF) => None,
            Err(e) => {
                self.valid_state = false;
                Some(Err(e))
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn it_works() {
        let data = &include_bytes!("warc.in")[..];

        let mut warc = WarcReader::new(data);

        let item = warc.next();
        assert!(item.is_some());
        let item = item.unwrap();
        assert!(item.is_ok());
        let item = item.unwrap();
        assert_eq!(
            item.header.get(&"WARC-Type".into()),
            Some(&"warcinfo".into())
        );

        let item = warc.next();
        assert!(item.is_some());
        let item = item.unwrap();
        assert!(item.is_ok());
        let item = item.unwrap();
        assert_eq!(
            item.header.get(&"WARC-Type".into()),
            Some(&"request".into())
        );

        let item = warc.next();
        assert!(item.is_some());
        let item = item.unwrap();
        assert!(item.is_err()); // incomplete record
    }
}
