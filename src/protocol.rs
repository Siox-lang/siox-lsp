//! Content-Length framing around JSON-RPC, independent of language features.

use serde_json::Value;
use std::io::{self, BufRead, Read, Write};

// Transport safety bounds, not compiler/type limits.
const MAX_HEADER: u64 = 8192;
const MAX_BODY: usize = 64 * 1024 * 1024;

pub fn read(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut length = None;
    let mut used = 0;
    loop {
        let mut line = String::new();
        let n = reader.take(MAX_HEADER - used + 1).read_line(&mut line)?;
        used += n as u64;
        if used > MAX_HEADER {
            return Err(invalid("LSP header exceeds 8 KiB"));
        }
        if n == 0 {
            return if used == 0 {
                Ok(None)
            } else {
                Err(invalid("truncated LSP header"))
            };
        }
        if line == "\r\n" {
            break;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| invalid("invalid LSP header"))?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(invalid("duplicate Content-Length"));
            }
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| invalid("invalid Content-Length"))?,
            );
        }
    }
    let length = length.ok_or_else(|| invalid("missing Content-Length"))?;
    if length > MAX_BODY {
        return Err(invalid("LSP message exceeds 64 MiB"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

pub fn write(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    #[test]
    fn unicode_frames_and_eof() {
        let message = json!({"value": "😀é"});
        let mut wire = Vec::new();
        write(&mut wire, &message).unwrap();
        write(&mut wire, &json!(null)).unwrap();
        let mut reader = Cursor::new(wire);
        assert_eq!(
            serde_json::from_slice::<Value>(&read(&mut reader).unwrap().unwrap()).unwrap(),
            message
        );
        assert_eq!(read(&mut reader).unwrap().unwrap(), b"null");
        assert!(read(&mut reader).unwrap().is_none());
    }

    #[test]
    fn rejects_bad_frames() {
        for frame in [
            "Content-Length: x\r\n\r\n",
            "Content-Length: 1\r\nContent-Length: 1\r\n\r\nx",
            "\r\n",
            "Content-Length: 2\r\n\r\nx",
            "Content-Length: 999999999\r\n\r\n",
            "Content-Length: 1\r\n",
            "Content-Length: 1\n\nx",
        ] {
            assert!(read(&mut Cursor::new(frame)).is_err(), "{frame}");
        }
        let oversized = format!("X-Header: {}\r\n", "a".repeat(MAX_HEADER as usize));
        assert!(read(&mut Cursor::new(oversized)).is_err());
    }
}
