//! The newer dongle (046D:0ADF): CRC-16/XMODEM framed messages on the vendor collection
//! FF00:0001 (unnumbered 65-byte reports); "PA" commands inside them reach the headset's radio
//! chip. A session opens with 84 01 / 85 01, routes the next command to the headset (wireless
//! GET_MODE / SET_MODE 01 / GET_MODE), asks OTA GET_BATTERY and closes with 85 02. A read does
//! not interrupt audio or the microphone (checked on hardware by the TaskbarVitals port of
//! this protocol).
//!
//! Protocol from portyporty/g435-battery-percentage (MIT); ported from BatteryHub's driver and
//! NOT tested here against this dongle (no such dongle at hand) - treat it as unverified.
use crate::hid::HidDev;
use crate::protocol::parse_adf_mv;
use crate::util::Deadline;
use crate::Reading;

const CAT_OTA: u8 = 0x06;
const CAT_WIRELESS: u8 = 0x0E;
const GET_MODE: u8 = 0xE0;
const SET_MODE: u8 = 0xE1;
/// Session init from the capture (OTA IOCTL_READ_MAX_LEN, sequence 1 inside).
const SESSION_INIT: [u8; 19] = [0x50, 0x41, 0x06, 0x01, 0x0D, 0x00, 0x25, 0x34, 0x12, 0x5A, 0x5A, 0x01, 0x00, 0x00, 0x00, 0xF0, 0x00, 0x00, 0x00];

/// CRC-16/XMODEM (poly 1021, init 0).
fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// One 64-byte frame: <cmd> <body length> <crc lo> <crc hi> <body ..>, the CRC computed over
/// the frame with the CRC field zeroed.
fn frame(cmd: u8, body: &[u8]) -> [u8; 64] {
    let mut f = [0u8; 64];
    f[0] = cmd;
    f[1] = body.len() as u8;
    let n = body.len().min(60);
    f[4..4 + n].copy_from_slice(&body[..n]);
    let crc = crc16(&f);
    f[2] = (crc & 0xFF) as u8;
    f[3] = (crc >> 8) as u8;
    f
}

/// A "PA" command: 50 41 <category> <seq> <length> <payload ..>.
fn pa(category: u8, seq: u8, len: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0x50, 0x41, category, seq, len];
    v.extend_from_slice(payload);
    v
}

/// Sends one frame (report id 0 first) and reads for `wait_ms`; with `want_mv`, stops at a
/// battery reply. None = the transport failed; Some(None) = answered, but no battery reply.
fn transact(d: &HidDev, cmd: u8, body: &[u8], wait_ms: u64, want_mv: bool) -> Option<Option<i32>> {
    let f = frame(cmd, body);
    let mut out = Vec::with_capacity(65);
    out.push(0x00);
    out.extend_from_slice(&f);
    if !d.write(&out) {
        return None;
    }
    let end = Deadline::in_ms(wait_ms);
    while !end.passed() {
        if let Some(r) = d.read(end.left_ms())
            && want_mv
            && let Some(mv) = parse_adf_mv(&r)
        {
            return Some(Some(mv));
        }
    }
    Some(None)
}

pub(crate) fn read(d: &HidDev) -> Reading {
    // 84 01 opens the session; only then is there something to close again.
    if transact(d, 0x84, &[0x01], 50, false).is_none() {
        return Reading::Failed("the dongle did not acknowledge the session open");
    }
    let mut seq: u8 = 2;
    let mut next = |cat: u8, len: u8, payload: &[u8]| {
        let cmd = pa(cat, seq, len, payload);
        seq = seq % 127 + 1;
        cmd
    };
    let steps = (|| {
        transact(d, 0x85, &[0x01], 50, false)?;
        transact(d, 0x80, &SESSION_INIT, 450, false)?;
        transact(d, 0x80, &next(CAT_WIRELESS, 1, &[GET_MODE]), 120, false)?;
        transact(d, 0x80, &next(CAT_WIRELESS, 2, &[SET_MODE, 0x01]), 120, false)?;
        transact(d, 0x80, &next(CAT_WIRELESS, 1, &[GET_MODE]), 120, false)?;
        transact(d, 0x80, &next(CAT_OTA, 1, &[0x00, 0x31]), 250, true)
    })();
    // The session is always closed once it was opened.
    let _ = transact(d, 0x85, &[0x02], 50, false);
    match steps {
        Some(Some(mv)) => Reading::Millivolts(mv),
        Some(None) => Reading::Off,
        None => Reading::Failed("the dongle stopped answering mid-session"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames() {
        assert_eq!(crc16(b"123456789"), 0x31C3); // CRC-16/XMODEM check value
        let f = frame(0x84, &[0x01]);
        assert_eq!(&f[..2], &[0x84, 0x01]);
        assert_eq!(f[4], 0x01);
        let mut z = f;
        z[2] = 0;
        z[3] = 0;
        assert_eq!(crc16(&z), u16::from_le_bytes([f[2], f[3]]));
        assert_eq!(pa(CAT_OTA, 7, 1, &[0x00, 0x31]), vec![0x50, 0x41, 0x06, 7, 1, 0x00, 0x31]);
    }
}
