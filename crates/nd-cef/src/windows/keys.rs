//! Windows's half of the key conversion: the `WM_KEYDOWN` `lParam` Chromium reads the scan code
//! (and so the DOM `code`) from on a US layout, and the wheel step of Chromium on Windows.

/// Pixels a wheel line scrolls: Chromium on Windows scrolls 100 px for a notch of three lines.
/// GPUI reports a notch as the system's lines per notch.
pub(crate) const PIXELS_PER_LINE: f32 = 100. / 3.;

/// The key's `WM_KEYDOWN` `lParam` on a US layout (repeat count 1, scan code in bits 16 to 23,
/// the extended bit 24 for the navigation block); 0 when unknown.
pub(crate) fn native_key(key: &str) -> i32 {
    match scan_code(key) {
        Some((scan, extended)) => 1 | (scan << 16) | (i32::from(extended) << 24),
        None => 0,
    }
}

/// The set 1 scan code of a GPUI key name, and whether it is an extended (`E0`) key.
fn scan_code(key: &str) -> Option<(i32, bool)> {
    const ROWS: [(&str, i32); 4] =
        [("1234567890-=", 0x02), ("qwertyuiop[]", 0x10), ("asdfghjkl;'`", 0x1E), ("zxcvbnm,./", 0x2C)];
    const SHIFTED: [(&str, &str); 4] =
        [("!@#$%^&*()_+", "1234567890-="), ("{}", "[]"), (":\"~", ";'`"), ("<>?", ",./")];
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let c = c.to_ascii_lowercase();
        let c = SHIFTED.iter().find_map(|(from, to)| from.find(c).and_then(|i| to.chars().nth(i))).unwrap_or(c);
        if c == '\\' || c == '|' {
            return Some((0x2B, false));
        }
        return ROWS.iter().find_map(|(row, first)| row.find(c).map(|i| (first + i as i32, false)));
    }
    Some(match key {
        "escape" => (0x01, false),
        "backspace" => (0x0E, false),
        "tab" => (0x0F, false),
        "enter" => (0x1C, false),
        "space" => (0x39, false),
        "home" => (0x47, true),
        "up" => (0x48, true),
        "pageup" => (0x49, true),
        "left" => (0x4B, true),
        "right" => (0x4D, true),
        "end" => (0x4F, true),
        "down" => (0x50, true),
        "pagedown" => (0x51, true),
        "insert" => (0x52, true),
        "delete" => (0x53, true),
        "menu" => (0x5D, true),
        _ => match key.strip_prefix('f').and_then(|n| n.parse::<i32>().ok()) {
            Some(n @ 1..=10) => (0x3A + n, false),
            Some(11) => (0x57, false),
            Some(12) => (0x58, false),
            _ => return None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_us_scan_codes() {
        assert_eq!(native_key("a"), 1 | (0x1E << 16));
        assert_eq!(native_key("q"), 1 | (0x10 << 16));
        assert_eq!(native_key("!"), 1 | (0x02 << 16));
        assert_eq!(native_key("escape"), 1 | (0x01 << 16));
        assert_eq!(native_key("f1"), 1 | (0x3B << 16));
        assert_eq!(native_key("f12"), 1 | (0x58 << 16));
        assert_eq!(native_key("\\"), 1 | (0x2B << 16));
        assert_eq!(native_key("left"), 1 | (0x4B << 16) | (1 << 24));
        assert_eq!(native_key("nope"), 0);
    }
}
