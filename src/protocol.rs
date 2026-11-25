//! Pure protocol pieces: reply parsing and the voltage curve. Unit-tested against frames
//! captured from real dongles.

/// Voltage -> charge in percent: the Li-Po discharge curve of g435-battery-percentage,
/// piecewise linear from 4200 mV = 100 % down to 3400 mV = 0 %.
pub fn mv_to_percent(mv: i32) -> i32 {
    const V: [i32; 16] = [4200, 4150, 4100, 4050, 4000, 3950, 3900, 3850, 3800, 3750, 3700, 3650, 3600, 3550, 3500, 3400];
    const P: [i32; 16] = [100, 95, 90, 85, 80, 75, 70, 62, 55, 45, 35, 25, 15, 10, 5, 0];
    if mv >= V[0] {
        return 100;
    }
    for i in 1..16 {
        if mv >= V[i] {
            return P[i] + ((P[i - 1] - P[i]) * (mv - V[i]) + (V[i - 1] - V[i]) / 2) / (V[i - 1] - V[i]);
        }
    }
    0
}

pub(crate) fn find(f: &[u8], pat: &[u8]) -> Option<usize> {
    f.windows(pat.len()).position(|w| w == pat)
}

/// Battery voltage from a GET_BATTERY reply of the newer dongle (input reports on FF00:0001);
/// None when absent or not a plausible Li-Po voltage.
pub(crate) fn parse_adf_mv(r: &[u8]) -> Option<i32> {
    r.windows(8).find_map(|w| {
        (w[0] == 0x00 && w[1] == 0x07 && w[2] == 0x00 && w[3] == 0x06 && w[5] == 0x00)
            .then(|| w[6] as i32 | (w[7] as i32) << 8)
            .filter(|mv| (2500..=4500).contains(mv))
    })
}

/// Battery voltage from a GET_BATTERY reply parked by the older dongle's bridge in feature
/// report 6; None when absent or implausible (dongles still in remote mode 0 answer with a
/// junk voltage, e.g. 234 mV).
pub(crate) fn parse_acb_mv(f: &[u8]) -> Option<i32> {
    find(f, &[0x07, 0x00, 0x06]).and_then(|i| {
        let w = f.get(i..i + 8)?;
        (w[3] & 0x80 != 0 && w[4] == 0x00)
            .then(|| w[5] as i32 | (w[6] as i32) << 8)
            .filter(|mv| (2500..=4600).contains(mv))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames captured from a real 0ACB dongle: a mode-0 battery answer is junk (234 mV), the
    /// answer after switching to remote mode 1 holds the level (4050 mV).
    #[test]
    fn acb_replies() {
        let junk = [0x06u8, 0x50, 0x49, 0x01, 0xC0, 0x39, 0x84, 0x2D, 0x02, 0x07, 0x00, 0x06, 0x82, 0x00, 0xEA, 0x00, 0x00, 0x00];
        assert_eq!(parse_acb_mv(&junk), None);
        let real = [0x06u8, 0x50, 0x49, 0x01, 0xC3, 0xBC, 0xFB, 0x17, 0x02, 0x07, 0x00, 0x06, 0x84, 0x00, 0xD2, 0x0F, 0x00, 0x00];
        assert_eq!(parse_acb_mv(&real), Some(4050));
        let status = [0x06u8, 0x50, 0x49, 0x0E, 0xC2, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0xE3, 0x01];
        let i = find(&status, &[0x02, 0x00, 0xE3]).unwrap();
        assert_eq!(status[i + 3], 1);
    }

    /// A GET_BATTERY reply of the newer dongle, as captured in g435-battery-percentage.
    #[test]
    fn adf_replies() {
        let mut r = vec![0u8; 64];
        r[10..18].copy_from_slice(&[0x00, 0x07, 0x00, 0x06, 0x09, 0x00, 0x68, 0x0F]); // 3944 mV
        assert_eq!(parse_adf_mv(&r), Some(3944));
        r[16] = 0x10;
        r[17] = 0x00; // 16 mV: not a voltage
        assert_eq!(parse_adf_mv(&r), None);
    }

    #[test]
    fn curve() {
        assert_eq!(mv_to_percent(4250), 100);
        assert_eq!(mv_to_percent(3300), 0);
        assert_eq!(mv_to_percent(4050), 85);
        assert_eq!(mv_to_percent(3800), 55);
        assert!(mv_to_percent(3944) > 70 && mv_to_percent(3944) < 75);
    }
}
