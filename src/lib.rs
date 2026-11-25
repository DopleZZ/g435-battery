//! Battery level of the Logitech G435 LIGHTSPEED headset over its 2.4 GHz USB dongle - a
//! small standalone Rust crate for Windows.
//!
//! Works with both dongles the headset shipped with:
//!
//! | Dongle | Transport | Audio |
//! |---|---|---|
//! | `046D:0ACB` (older) | bootloader bridge on the vendor collection `FFC0`, report 6, feature reports | a session stops the dongle's own audio for ~0.6 s, so [`read()`] runs one only while its playback and microphone are silent ([`audio_busy`]) |
//! | `046D:0ADF` (newer) | CRC-16 framed messages on `FF00:0001`, unnumbered 65-byte reports | a read does not interrupt audio |
//!
//! The dongle answers with the battery voltage in millivolts; [`mv_to_percent`] maps it onto a
//! Li-Po discharge curve. No caching, no threads - call it on your own schedule.
//!
//! Protocol facts: [deddroid/logitech-g435-battery](https://github.com/deddroid/logitech-g435-battery)
//! (MIT) for `0ACB`, verified on hardware on Windows 10; [portyporty/g435-battery-percentage](https://github.com/portyporty/g435-battery-percentage)
//! (MIT) for `0ADF`, ported but not verified here (no such dongle at hand). The HID/transport
//! code is adapted from [BatteryHub](https://github.com/ferloom/Wireless-Device-Battery-Monitor)
//! (GPL-3.0), hence this crate's license.
//!
//! # Example
//! ```no_run
//! match g435_battery::read() {
//!     g435_battery::Reading::Millivolts(mv) => {
//!         println!("G435: {mv} mV ≈ {}%", g435_battery::mv_to_percent(mv));
//!     }
//!     g435_battery::Reading::Off => println!("headset off or out of range"),
//!     g435_battery::Reading::AudioBusy => println!("audio playing - read skipped"),
//!     g435_battery::Reading::NotFound => println!("no G435 dongle"),
//!     g435_battery::Reading::Failed(why) => println!("read failed: {why}"),
//! }
//! ```
//!
//! Troubleshooting a dongle: set `G435_BATTERY_DEBUG=1` to get a trace on stderr.

mod acb;
mod adf;
mod audio;
mod hid;
mod protocol;
mod util;

pub use protocol::mv_to_percent;

/// The outcome of one read attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// The battery voltage in millivolts (a plausible Li-Po voltage).
    Millivolts(i32),
    /// The dongle answered, the headset is switched off or out of range.
    Off,
    /// Older dongle only: its playback or microphone is in use, so the read was skipped - a
    /// session would stop its audio for ~0.6 s. Retry when it goes silent.
    AudioBusy,
    /// No G435 dongle is present.
    NotFound,
    /// The dongle is present but did not answer the protocol.
    Failed(&'static str),
}

/// Reads the headset's battery from whichever dongle is present. On the older dongle the read
/// is skipped while its audio endpoints carry a signal ([`Reading::AudioBusy`]); use
/// [`read_ungated`] to read regardless of the audio.
pub fn read() -> Reading {
    match hid::find_dongle() {
        hid::G435::Adf(d) => adf::read(&d),
        hid::G435::Acb(d) => {
            if audio::busy("g435") {
                Reading::AudioBusy
            } else {
                acb::read(&d)
            }
        }
        hid::G435::Missing => Reading::NotFound,
    }
}

/// [`read()`] without the audio gate: on the older dongle this stops its audio for ~0.6 s.
pub fn read_ungated() -> Reading {
    match hid::find_dongle() {
        hid::G435::Adf(d) => adf::read(&d),
        hid::G435::Acb(d) => acb::read(&d),
        hid::G435::Missing => Reading::NotFound,
    }
}

/// True while any active audio endpoint whose FriendlyName contains `name` (case-insensitive) -
/// playback or microphone - carries a signal, checked over ~120 ms. Also true when the meters
/// cannot be read: never cut audio unchecked.
pub fn audio_busy(name: &str) -> bool {
    audio::busy(name)
}

/// The level as percent (0..=100); `None` unless the read returned a voltage.
pub fn read_percent() -> Option<i32> {
    match read() {
        Reading::Millivolts(mv) => Some(mv_to_percent(mv)),
        _ => None,
    }
}
