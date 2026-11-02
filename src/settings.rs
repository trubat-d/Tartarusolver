//! The overlay's settings, kept in settings.json next to the run's state: the game log chosen by hand, the hotkeys and
//! the boxes' color.

use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// The game log chosen by hand (None: found through Steam).
    pub log: Option<String>,
    pub boxes_key: String,
    pub redo_key: String,
    pub quit_key: String,
    /// The boxes' color, red, green, blue.
    pub color: [u8; 3],
    /// The start-up notice was read and "Don't show this again" ticked.
    pub hide_notice: bool,
    /// The version whose terms were last accepted: a new version asks again.
    pub accepted: Option<String>,
    /// Let screenshots and recordings show the boxes (off: the boxes are kept out of captures).
    pub boxes_in_captures: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            log: None,
            boxes_key: "Ctrl+Alt+B".into(),
            redo_key: "Ctrl+Alt+R".into(),
            quit_key: "Ctrl+Alt+Q".into(),
            color: [40, 255, 90],
            hide_notice: false,
            accepted: None,
            boxes_in_captures: false,
        }
    }
}

impl Settings {
    /// From settings.json; anything missing or wrong keeps its default.
    pub fn from_json(v: &Value) -> Settings {
        let d = Settings::default();
        let key = |k: &str, def: &str| {
            v[k].as_str()
                .filter(|s| parse_hotkey(s).is_some())
                .unwrap_or(def)
                .to_string()
        };
        Settings {
            log: v["log"].as_str().filter(|s| !s.is_empty()).map(String::from),
            boxes_key: key("boxes_key", &d.boxes_key),
            redo_key: key("redo_key", &d.redo_key),
            quit_key: key("quit_key", &d.quit_key),
            color: v["color"].as_str().and_then(parse_color).unwrap_or(d.color),
            hide_notice: v["hide_notice"].as_bool().unwrap_or(false),
            accepted: v["accepted"].as_str().map(String::from),
            boxes_in_captures: v["boxes_in_captures"].as_bool().unwrap_or(false),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "log": self.log,
            "boxes_key": self.boxes_key,
            "redo_key": self.redo_key,
            "quit_key": self.quit_key,
            "color": format_color(self.color),
            "hide_notice": self.hide_notice,
            "accepted": self.accepted,
            "boxes_in_captures": self.boxes_in_captures,
        })
    }
}

/// A hotkey as typed ("Ctrl+Alt+B", "Shift+F7"): (Ctrl, Alt, Shift, Win, Windows virtual-key code). The key is a
/// letter, a digit or F1-F24, with at least one of Ctrl, Alt, Win (or Shift with an F key).
pub fn parse_hotkey(s: &str) -> Option<(bool, bool, bool, bool, u32)> {
    let (mut ctrl, mut alt, mut shift, mut win) = (false, false, false, false);
    let mut key = None;
    for part in s.split('+').map(|p| p.trim()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" => alt = true,
            "shift" => shift = true,
            "win" | "windows" => win = true,
            "" => return None,
            k => {
                if key.is_some() {
                    return None;
                }
                let up = k.to_ascii_uppercase();
                key = if up.len() == 1 && up.chars().all(|c| c.is_ascii_alphanumeric()) {
                    Some(up.chars().next().unwrap() as u32)
                } else if let Some(n) = up.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
                    (1..=24).contains(&n).then_some(0x70 + n - 1)
                } else {
                    return None;
                };
            }
        }
    }
    let key = key?;
    let fkey = key >= 0x70;
    (ctrl || alt || win || (shift && fkey) || fkey).then_some((ctrl, alt, shift, win, key))
}

/// "#RRGGBB" (the # optional).
pub fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?])
}

pub fn format_color(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_and_colors() {
        assert_eq!(parse_hotkey("Ctrl+Alt+B"), Some((true, true, false, false, 'B' as u32)));
        assert_eq!(parse_hotkey("ctrl + shift + 7"), Some((true, false, true, false, '7' as u32)));
        assert_eq!(parse_hotkey("F8"), Some((false, false, false, false, 0x77)));
        assert_eq!(parse_hotkey("B"), None);
        assert_eq!(parse_hotkey("Ctrl+Alt+Space"), None);
        assert_eq!(parse_hotkey("Ctrl+A+B"), None);
        assert_eq!(parse_color("#28FF5A"), Some([0x28, 0xFF, 0x5A]));
        assert_eq!(parse_color("zz"), None);
        let s = Settings { log: Some("D:\\x\\UE_game.log".into()), quit_key: "F9".into(), ..Default::default() };
        assert_eq!(Settings::from_json(&s.to_json()), s);
        assert_eq!(Settings::from_json(&json!({"boxes_key": "nope"})), Settings::default());
    }
}
