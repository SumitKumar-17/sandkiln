//! Per-clone state repair after a snapshot restore (`Request::ResumeFixup`).
//!
//! A restored guest keeps the entropy pool and wall clock it had at
//! snapshot time. This mixes fresh host entropy into the kernel pool,
//! forces a reseed so the next random bytes differ from every other clone
//! of the same snapshot, and sets the wall clock from the host.

use base64::Engine;
use std::os::fd::AsRawFd;

/// Linux `RNDADDENTROPY`: `_IOW('R', 0x03, int[2])`.
const RNDADDENTROPY: libc::c_ulong = 0x4008_5203;
/// Linux `RNDRESEEDCRNG`: `_IO('R', 0x07)`.
const RNDRESEEDCRNG: libc::c_ulong = 0x5207;

/// The `struct rand_pool_info` the kernel expects: entropy credit in bits,
/// buffer length in bytes, then the bytes. Crediting is correct here: the
/// bytes come from the host's CSPRNG.
pub fn rand_pool_info_bytes(entropy: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + entropy.len());
    out.extend_from_slice(&((entropy.len() as i32) * 8).to_ne_bytes());
    out.extend_from_slice(&(entropy.len() as i32).to_ne_bytes());
    out.extend_from_slice(entropy);
    out
}

pub fn apply(unix_secs: u64, unix_nanos: u32, entropy_base64: &str) -> Result<(), String> {
    let entropy = base64::engine::general_purpose::STANDARD
        .decode(entropy_base64)
        .map_err(|e| format!("bad entropy: {e}"))?;
    reseed(&entropy)?;
    set_clock(unix_secs, unix_nanos)
}

fn reseed(entropy: &[u8]) -> Result<(), String> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/urandom")
        .map_err(|e| format!("open /dev/urandom: {e}"))?;
    let info = rand_pool_info_bytes(entropy);
    // SAFETY: `info` is a valid `rand_pool_info` of the length it declares.
    if unsafe { libc::ioctl(file.as_raw_fd(), RNDADDENTROPY as _, info.as_ptr()) } != 0 {
        return Err(format!("RNDADDENTROPY: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: no argument; the ioctl only triggers a reseed.
    if unsafe { libc::ioctl(file.as_raw_fd(), RNDRESEEDCRNG as _) } != 0 {
        return Err(format!("RNDRESEEDCRNG: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

fn set_clock(secs: u64, nanos: u32) -> Result<(), String> {
    let ts = libc::timespec { tv_sec: secs as _, tv_nsec: nanos as libc::c_long };
    // SAFETY: `ts` is a valid timespec for the duration of the call.
    if unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &ts) } != 0 {
        return Err(format!("clock_settime: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_info_credits_eight_bits_per_byte_and_carries_the_bytes() {
        let bytes = rand_pool_info_bytes(&[1, 2, 3, 4]);
        assert_eq!(bytes.len(), 12);
        assert_eq!(i32::from_ne_bytes(bytes[0..4].try_into().unwrap()), 32);
        assert_eq!(i32::from_ne_bytes(bytes[4..8].try_into().unwrap()), 4);
        assert_eq!(&bytes[8..], &[1, 2, 3, 4]);
    }

    #[test]
    fn bad_base64_is_rejected_before_touching_the_system() {
        assert!(apply(1, 0, "!!!not base64").unwrap_err().contains("bad entropy"));
    }
}
