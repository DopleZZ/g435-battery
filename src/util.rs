//! Small shared helpers: a deadline and an opt-in debug print.
use std::time::{Duration, Instant};

pub(crate) struct Deadline(Instant);

impl Deadline {
    pub(crate) fn in_ms(ms: u64) -> Self {
        Self(Instant::now() + Duration::from_millis(ms))
    }
    pub(crate) fn left_ms(&self) -> u32 {
        self.0.saturating_duration_since(Instant::now()).as_millis() as u32
    }
    pub(crate) fn passed(&self) -> bool {
        Instant::now() >= self.0
    }
}

pub(crate) fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Debug tracing behind G435_BATTERY_DEBUG=1, for troubleshooting a dongle.
pub(crate) fn dbg(msg: &str) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("G435_BATTERY_DEBUG").is_some()) {
        eprintln!("[g435-battery] {msg}");
    }
}
