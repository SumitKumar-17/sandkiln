use std::io::{self, Read, Write};

/// Wire format: a 4-byte little-endian length prefix followed by that many
/// bytes of JSON. Framing this way (rather than newline-delimited) means
/// command output containing arbitrary bytes never corrupts the stream.
const MAX_MESSAGE_LEN: u32 = 64 * 1024 * 1024;

pub fn read_message(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_MESSAGE_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

pub fn write_message(w: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    let len = payload.len() as u32;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn roundtrip(payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        write_message(&mut buf, payload).unwrap();
        read_message(&mut Cursor::new(buf)).unwrap()
    }

    #[test]
    fn roundtrips_arbitrary_bytes() {
        assert_eq!(roundtrip(b"hello"), b"hello");
    }

    #[test]
    fn roundtrips_empty_payload() {
        assert_eq!(roundtrip(b""), b"");
    }

    #[test]
    fn roundtrips_bytes_that_look_like_frame_boundaries() {
        // The whole point of length-prefixed framing over newline-delimited:
        // a payload containing '\n', embedded nulls, or anything else must
        // never be misread as a boundary.
        let payload = b"line one\nline two\x00\xff\xfe";
        assert_eq!(roundtrip(payload), payload);
    }

    #[test]
    fn write_message_prefixes_exact_little_endian_length() {
        let mut buf = Vec::new();
        write_message(&mut buf, b"abc").unwrap();
        assert_eq!(&buf[0..4], &3u32.to_le_bytes());
        assert_eq!(&buf[4..], b"abc");
    }

    #[test]
    fn read_message_rejects_oversized_length_prefix() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(MAX_MESSAGE_LEN + 1).to_le_bytes());
        let err = read_message(&mut Cursor::new(buf)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn read_message_fails_cleanly_on_truncated_input() {
        // A length prefix claiming more bytes than are actually present —
        // must error, not hang or panic (Cursor's Read impl returns
        // UnexpectedEof from read_exact in this case).
        let mut buf = Vec::new();
        buf.extend_from_slice(&10u32.to_le_bytes());
        buf.extend_from_slice(b"abc"); // only 3 of the promised 10 bytes
        let err = read_message(&mut Cursor::new(buf)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn read_message_fails_cleanly_on_missing_length_prefix() {
        let err = read_message(&mut Cursor::new(vec![0u8, 1, 2])).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
