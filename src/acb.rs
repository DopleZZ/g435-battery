//! The older dongle (046D:0ACB): the battery is readable only through its bootloader bridge.
//!
//! enter (06 01 03), "PA" commands to the headset's radio chip wrapped in 06 04 03 <len16>,
//! replies parked in feature report 6, leave (06 08 03) - each bridge message is acknowledged
//! from the input pipe with 06 5A (or 06 FE, "already in"). While the bridge is up the dongle's
//! own audio is stopped (~0.6 s per session), so callers gate reads on silence themselves
//! ([`crate::audio_busy`]).
//!
//! GET_BATTERY is never the session's first command: dongles still in remote mode 0 answer it
//! with a junk voltage, and the reference implementation reports the dongle hanging outright
//! unless a wireless command went first. The connect status goes first, and the mode is
//! switched to 1 only when the first battery answer is implausible (it persists on the dongle,
//! so later sessions take the direct path).
//!
//! Protocol from deddroid/logitech-g435-battery (MIT), verified on hardware against this
//! dongle on Windows 10.
use crate::hid::HidDev;
use crate::protocol::{find, parse_acb_mv};
use crate::util::{dbg, sleep_ms, Deadline};
use crate::Reading;

const ENTER: [u8; 2] = [0x01, 0x03];
const LEAVE: [u8; 2] = [0x08, 0x03];
const PA_CONNECT: [u8; 6] = [0x50, 0x41, 0x0E, 0x01, 0x01, 0xE3];
const PA_BATTERY: [u8; 7] = [0x50, 0x41, 0x06, 0x02, 0x01, 0x00, 0x31];
const PA_SET_MODE: [u8; 7] = [0x50, 0x41, 0x0E, 0x03, 0x02, 0xE1, 0x01];
const PA_BATTERY_2: [u8; 7] = [0x50, 0x41, 0x06, 0x04, 0x01, 0x00, 0x31];

/// One bridge message on report 6, then the 06 5A / 06 FE acknowledgement from the input pipe.
fn bridge_msg(d: &HidDev, body: &[u8]) -> bool {
    let mut out = vec![0u8; d.out_len()];
    out[0] = 0x06;
    out[1..1 + body.len()].copy_from_slice(body);
    if !d.write(&out) {
        return false;
    }
    let end = Deadline::in_ms(900);
    while !end.passed() {
        if let Some(r) = d.read(end.left_ms())
            && r.len() >= 2
            && r[0] == 0x06
            && (r[1] == 0x5A || r[1] == 0xFE)
        {
            return true;
        }
    }
    false
}

/// A "PA" command, wrapped for the bridge.
fn bridge_pa(d: &HidDev, cmd: &[u8]) -> bool {
    let len = cmd.len() as u16;
    let mut body = vec![0x04, 0x03, len as u8, (len >> 8) as u8];
    body.extend_from_slice(cmd);
    bridge_msg(d, &body)
}

/// The bridge parks replies in feature report 6; FF in byte 1 means "nothing yet".
fn bridge_frame(d: &HidDev, wait_ms: u64) -> Option<Vec<u8>> {
    let end = Deadline::in_ms(wait_ms);
    loop {
        if let Some(f) = d.get_feature(0x06)
            && f.len() > 1
            && f[1] != 0xFF
        {
            return Some(f);
        }
        if end.passed() {
            return None;
        }
        sleep_ms(25);
    }
}

/// Drops pending replies (some commands also produce status frames).
fn bridge_drain(d: &HidDev, max: usize) {
    for _ in 0..max {
        match d.get_feature(0x06) {
            Some(f) if f.len() > 1 && f[1] != 0xFF => {}
            _ => return,
        }
    }
}

/// Waits for a battery reply. An implausible voltage passes through as None so the caller can
/// switch the mode and ask again.
fn acb_battery(d: &HidDev) -> Option<i32> {
    let end = Deadline::in_ms(1500);
    loop {
        let Some(f) = bridge_frame(d, end.left_ms() as u64) else { return None };
        if let Some(mv) = parse_acb_mv(&f) {
            return Some(mv);
        }
        if end.passed() {
            return None;
        }
    }
}

/// One bridge conversation, without enter/leave. Ok(Some(mv)) = level read, Ok(None) = the
/// headset is off or out of range (its connect status answered 0).
fn session(d: &HidDev) -> Result<Option<i32>, &'static str> {
    // Connect status first: GET_BATTERY must not be the first command (see module doc).
    if !bridge_pa(d, &PA_CONNECT) {
        return Err("the dongle did not acknowledge the connect-status command");
    }
    let end = Deadline::in_ms(1500);
    let connected = loop {
        let Some(f) = bridge_frame(d, end.left_ms() as u64) else { return Err("no connect-status reply") };
        if let Some(i) = find(&f, &[0x02, 0x00, 0xE3]) {
            break f.get(i + 3).copied().unwrap_or(0) != 0;
        }
        if end.passed() {
            return Err("no connect-status reply");
        }
    };
    bridge_drain(d, 4);
    if !connected {
        dbg("session: headset off");
        return Ok(None);
    }
    if !bridge_pa(d, &PA_BATTERY) {
        return Err("the dongle did not acknowledge the battery command");
    }
    let mv = match acb_battery(d) {
        Some(mv) => Some(mv),
        None => {
            // Junk or nothing: switch remote mode 1 (the state the battery query needs) and ask again.
            dbg("session: implausible first answer, switching remote mode 1");
            if !bridge_pa(d, &PA_SET_MODE) {
                return Err("the dongle did not acknowledge the mode command");
            }
            bridge_drain(d, 2);
            if !bridge_pa(d, &PA_BATTERY_2) {
                return Err("the dongle did not acknowledge the battery command");
            }
            acb_battery(d)
        }
    };
    Ok(mv)
}

/// Runs one bridge session and always leaves the bridge afterwards: while it is up, the
/// dongle's audio is dead. If leaving is not acknowledged it is retried on a fresh handle - a
/// bridge left up would keep the audio dead until the dongle is replugged.
pub(crate) fn read(d: &HidDev) -> Reading {
    if !bridge_msg(d, &ENTER) {
        return Reading::Failed("the dongle did not acknowledge entering the bridge");
    }
    let res = session(d);
    if !bridge_msg(d, &LEAVE) {
        dbg("leave: no ack, retrying on a fresh handle");
        sleep_ms(150);
        if let Some(d2) = d.reopen() {
            let _ = bridge_msg(&d2, &LEAVE);
        }
    }
    match res {
        Ok(Some(mv)) => {
            dbg(&format!("read: {mv} mV, {}%", crate::mv_to_percent(mv)));
            Reading::Millivolts(mv)
        }
        Ok(None) => Reading::Off,
        Err(why) => Reading::Failed(why),
    }
}
