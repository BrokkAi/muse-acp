//! Newline-delimited JSON-RPC framing, shared by the client and agent sides.

use std::io::{self, BufRead};

/// One newline-terminated frame, without its line ending, or `None` at EOF.
/// A frame longer than `max_bytes` is an `InvalidData` error.
pub(crate) fn read_frame<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();
    loop {
        let (found, consumed, chunk) = {
            let buffer = reader.fill_buf()?;
            if buffer.is_empty() {
                return if frame.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(frame))
                };
            }
            match buffer.iter().position(|byte| *byte == b'\n') {
                Some(position) => {
                    let chunk = buffer[..position].to_vec();
                    (true, position + 1, chunk)
                }
                None => (false, buffer.len(), buffer.to_vec()),
            }
        };
        if frame.len().saturating_add(chunk.len()) > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("ACP frame exceeds {max_bytes} bytes"),
            ));
        }
        frame.extend_from_slice(&chunk);
        reader.consume(consumed);
        if found {
            while matches!(frame.last(), Some(b'\r')) {
                frame.pop();
            }
            return Ok(Some(frame));
        }
    }
}
