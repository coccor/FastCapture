//! Hotkeys as written in `fastcapture.ini` (`Ctrl+Alt+Space`, `Win+Shift+C`), in the style of
//! FastPad's keymap, plus `Win`. Empty means off.

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyStroke {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub win: bool,
    /// The Win32 virtual-key code.
    pub vk: u16,
}

const NAMED: [(&str, u16); 18] = [
    ("Space", 0x20),
    ("Enter", 0x0D),
    ("Tab", 0x09),
    ("Esc", 0x1B),
    ("Backspace", 0x08),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("Home", 0x24),
    ("End", 0x23),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("PrintScreen", 0x2C),
    ("Pause", 0x13),
    ("Apps", 0x5D),
];

const ALIASES: [(&str, u16); 4] = [
    ("Return", 0x0D),
    ("Escape", 0x1B),
    ("Del", 0x2E),
    ("Ins", 0x2D),
];

fn key_from_name(name: &str) -> Option<u16> {
    if name.len() == 1 {
        let byte = name.as_bytes()[0].to_ascii_uppercase();
        if byte.is_ascii_uppercase() || byte.is_ascii_digit() {
            return Some(u16::from(byte));
        }
    }
    if let Some(number) = name
        .strip_prefix(['F', 'f'])
        .and_then(|digits| digits.parse::<u16>().ok())
        && (1..=24).contains(&number)
    {
        return Some(0x70 + number - 1);
    }
    NAMED
        .iter()
        .chain(&ALIASES)
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, vk)| *vk)
}

fn key_name(vk: u16) -> Option<String> {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => Some(char::from(vk as u8).to_string()),
        0x70..=0x87 => Some(format!("F{}", vk - 0x70 + 1)),
        _ => NAMED
            .iter()
            .find(|(_, known)| *known == vk)
            .map(|(name, _)| (*name).to_owned()),
    }
}

impl KeyStroke {
    /// `Ctrl+Alt+Space` and the like, ignoring case and spaces around `+`. `None` for an unknown
    /// key or modifier, or an empty text.
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let (key, modifiers) = parts.split_last()?;
        let mut stroke = Self {
            ctrl: false,
            shift: false,
            alt: false,
            win: false,
            vk: key_from_name(key)?,
        };
        for modifier in modifiers {
            let flag = match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut stroke.ctrl,
                "shift" => &mut stroke.shift,
                "alt" => &mut stroke.alt,
                "win" | "windows" => &mut stroke.win,
                _ => return None,
            };
            if *flag {
                return None;
            }
            *flag = true;
        }
        Some(stroke)
    }

    /// Whether the stroke has a modifier, as a global hotkey should.
    pub fn has_modifier(self) -> bool {
        self.ctrl || self.shift || self.alt || self.win
    }

    /// `RegisterHotKey`'s modifier bits (`MOD_ALT` 1, `MOD_CONTROL` 2, `MOD_SHIFT` 4, `MOD_WIN`
    /// 8), without `MOD_NOREPEAT`.
    pub fn hotkey_modifiers(self) -> u32 {
        u32::from(self.alt)
            | u32::from(self.ctrl) << 1
            | u32::from(self.shift) << 2
            | u32::from(self.win) << 3
    }
}

impl fmt::Display for KeyStroke {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = [
            (self.ctrl, "Ctrl"),
            (self.shift, "Shift"),
            (self.alt, "Alt"),
            (self.win, "Win"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| (*name).to_owned())
        .collect();
        parts.push(key_name(self.vk).unwrap_or_else(|| format!("{:#04x}", self.vk)));
        f.write_str(&parts.join("+"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_read_and_print_in_one_form() {
        let stroke = KeyStroke::parse("ctrl + alt + space").unwrap();
        assert_eq!(stroke.vk, 0x20);
        assert_eq!(stroke.to_string(), "Ctrl+Alt+Space");
        assert_eq!(stroke.hotkey_modifiers(), 0b0011);
        let stroke = KeyStroke::parse("Win+Shift+c").unwrap();
        assert_eq!(stroke.to_string(), "Shift+Win+C");
        assert_eq!(stroke.hotkey_modifiers(), 0b1100);
        assert_eq!(KeyStroke::parse("Ctrl+F12").unwrap().vk, 0x7B);
        assert_eq!(
            KeyStroke::parse("Alt+Escape").unwrap().to_string(),
            "Alt+Esc"
        );
        assert_eq!(KeyStroke::parse("Ctrl+Shift+5").unwrap().vk, 0x35);
        assert!(KeyStroke::parse("F1").is_some_and(|stroke| !stroke.has_modifier()));
    }

    #[test]
    fn unknown_or_repeated_parts_are_refused() {
        for bad in [
            "",
            "Ctrl+",
            "Hyper+A",
            "Ctrl+Ctrl+A",
            "Ctrl+F25",
            "Ctrl+AB",
            "Ctrl+é",
        ] {
            assert_eq!(KeyStroke::parse(bad), None, "{bad:?}");
        }
    }
}
