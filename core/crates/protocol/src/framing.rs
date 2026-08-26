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
