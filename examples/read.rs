//! Reads the battery of a Logitech G435 on either of its dongles and prints it.
fn main() {
    match g435_battery::read() {
        g435_battery::Reading::Millivolts(mv) => {
            println!("Logitech G435: {mv} mV ≈ {}%", g435_battery::mv_to_percent(mv));
        }
        g435_battery::Reading::Off => println!("Logitech G435: dongle present, headset off or out of range"),
        g435_battery::Reading::AudioBusy => println!("Logitech G435: audio playing - read skipped (the older dongle stops its audio during a read)"),
        g435_battery::Reading::NotFound => println!("No Logitech G435 dongle found"),
        g435_battery::Reading::Failed(why) => println!("Logitech G435: read failed ({why})"),
    }
}
