//! Linux's half of the key conversion: XKB keycodes for the DOM `code` of a US layout, and the
//! wheel step of Chromium's GTK port.

/// Pixels a wheel notch scrolls, as Chromium's GTK port counts it.
pub(crate) const PIXELS_PER_LINE: f32 = 40.;

/// The XKB keycode (evdev + 8) of the key on a US layout, from which Chromium derives the DOM
/// `code`; 0 when unknown.
pub(crate) fn native_key(key: &str) -> i32 {
    const ROWS: [(&str, i32); 4] =
        [("1234567890-=", 2), ("qwertyuiop[]", 16), ("asdfghjkl;'`", 30), ("zxcvbnm,./", 44)];
    const SHIFTED: [(&str, &str); 4] =
        [("!@#$%^&*()_+", "1234567890-="), ("{}", "[]"), (":\"~", ";'`"), ("<>?", ",./")];
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let c = c.to_ascii_lowercase();
        let c = SHIFTED.iter().find_map(|(from, to)| from.find(c).and_then(|i| to.chars().nth(i))).unwrap_or(c);
        if c == '\\' {
            return 43 + 8;
        }
        return ROWS.iter().find_map(|(row, first)| row.find(c).map(|i| first + i as i32 + 8)).unwrap_or(0);
    }
    let evdev = match key {
        "escape" => 1,
        "backspace" => 14,
        "tab" => 15,
        "enter" => 28,
        "space" => 57,
        "home" => 102,
        "up" => 103,
        "pageup" => 104,
        "left" => 105,
        "right" => 106,
        "end" => 107,
        "down" => 108,
        "pagedown" => 109,
        "insert" => 110,
        "delete" => 111,
        _ => match key.strip_prefix('f').and_then(|n| n.parse::<i32>().ok()) {
            Some(n @ 1..=10) => 58 + n,
            Some(11) => 87,
            Some(12) => 88,
            _ => return 0,
        },
    };
    evdev + 8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_us_keycodes() {
        assert_eq!(native_key("a"), 38);
        assert_eq!(native_key("q"), 24);
        assert_eq!(native_key("1"), 10);
        assert_eq!(native_key("!"), 10);
        assert_eq!(native_key("escape"), 9);
        assert_eq!(native_key("f12"), 96);
        assert_eq!(native_key("\\"), 51);
    }
}
