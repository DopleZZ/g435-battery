# g435-battery

Battery level of the **Logitech G435** headset over its 2.4 GHz USB dongle. A small standalone Rust crate for Windows - no G HUB, no background service, one function call.

The G435 shipped with two different dongles; this crate reads the battery through either:

| Dongle | How | Notes |
|---|---|---|
| `046D:0ACB` (older) | bootloader bridge on the dongle's vendor HID collection `FFC0` (report 6, feature reports) | a session in the bridge **stops the dongle's own audio for ~0.6 s**, so [`read()`] first checks the dongle's playback and microphone with WASAPI peak meters and skips the read while they carry a signal |
| `046D:0ADF` (newer) | CRC-16 framed messages on `FF00:0001` | a read does not interrupt audio |

The level arrives as the battery voltage in millivolts; [`mv_to_percent()`] maps it onto a Li-Po discharge curve.

## Usage

```toml
[dependencies]
g435-battery = { git = "https://github.com/DopleZZ/g435-battery" }
```

```rust
match g435_battery::read() {
    g435_battery::Reading::Millivolts(mv) => {
        println!("G435: {mv} mV ≈ {}%", g435_battery::mv_to_percent(mv));
    }
    g435_battery::Reading::Off => println!("headset off or out of range"),
    g435_battery::Reading::AudioBusy => println!("audio playing - read skipped"),
    g435_battery::Reading::NotFound => println!("no G435 dongle"),
    g435_battery::Reading::Failed(why) => println!("read failed: {why}"),
}
```

Try it against a plugged-in dongle:

```
cargo run --example read
Logitech G435: 3948 mV ≈ 75%
```

The API is synchronous and does no caching - call it on your own schedule (every few minutes is plenty). The older dongle's `remote mode` is switched to 1 on first read and persists on the dongle, so later sessions take the direct path.

## Reading safely around audio (older dongle)

The `0ACB` dongle only answers its battery query from bootloader mode, and while it is there its own audio stream is stopped - this is firmware behavior, not something a driver can turn off. This crate makes the interruption a non-event:

- [`read()`] checks the dongle's playback **and** microphone peak meters right before a session; if either carries a signal it returns [`Reading::AudioBusy`] instead and leaves the dongle alone.
- [`read_ungated()`] reads regardless - know what you are doing (a ~0.6 s gap).
- A session is entered only after a connect-status query (a battery query as the first command can hang dongles still in remote mode 0), and the bridge is always left again, retried on a fresh handle if the exit is not acknowledged.

Known trade-offs: sound that *starts* in the milliseconds between the check and the bridge entry can be cut once (one ~0.6 s gap); audio quieter than roughly −48 dB counts as silence; while audio flows continuously, the level simply is not refreshed - schedule retries.

## Protocol & credits

- `046D:0ACB`: [deddroid/logitech-g435-battery](https://github.com/deddroid/logitech-g435-battery) (MIT) - the bridge transport, command set and plausibility rules, verified on hardware here on Windows 10.
- `046D:0ADF`: [portyporty/g435-battery-percentage](https://github.com/portyporty/g435-battery-percentage) (MIT) - the CRC framing and session; ported from BatteryHub's driver, **not verified here** (no such dongle at hand). If you have one, `cargo run --example read` and a PR with the outcome would be welcome.
- The HID/transport scaffolding is adapted from [BatteryHub](https://github.com/ferloom/Wireless-Device-Battery-Monitor), where the same driver also lives as a first-class device driver.

Set `G435_BATTERY_DEBUG=1` for a stderr trace when troubleshooting a dongle.

## License

GPL-3.0-or-later. The protocol facts come from MIT-licensed projects (see credits), but this crate's HID/transport code is adapted from GPL-3.0 BatteryHub, so the crate as a whole is GPL.
