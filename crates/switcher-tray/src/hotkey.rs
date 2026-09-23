//! Hotkeys as the configuration writes them, turned into what Windows takes.
//!
//! The tray registers every action's hotkey itself. It is running anyway, a
//! registered hotkey fires instantly, and it needs nothing in the Start Menu —
//! Windows only honours a *shortcut's* hotkey while the shortcut sits there,
//! which is how the previous installer did it, and why the Start Menu filled
//! up with one entry per action.
//!
//! Pure, so the parsing is tested on every platform.

use switcher_core::config::Config;

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;

/// A key combination: modifier flags and a virtual-key code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub key: u32,
}

/// `CTRL+ALT+1`, `F24`, `shift+f24`: modifiers and exactly one key.
pub fn parse(text: &str) -> Result<Hotkey, String> {
    let mut modifiers = 0;
    let mut key = None;
    for part in text.split('+').map(str::trim) {
        if part.is_empty() {
            return Err(format!("`{text}` has an empty part"));
        }
        let upper = part.to_ascii_uppercase();
        let modifier = match upper.as_str() {
            "CTRL" | "CONTROL" => Some(MOD_CONTROL),
            "ALT" => Some(MOD_ALT),
            "SHIFT" => Some(MOD_SHIFT),
            "WIN" | "SUPER" | "META" => Some(MOD_WIN),
            _ => None,
        };
        match modifier {
            Some(m) => modifiers |= m,
            None if key.is_some() => {
                return Err(format!("`{text}` names two keys; a hotkey has one"))
            }
            None => {
                key =
                    Some(virtual_key(&upper).ok_or_else(|| {
                        format!("`{part}` in `{text}` is not a key this understands")
                    })?)
            }
        }
    }
    let key = key.ok_or_else(|| format!("`{text}` has modifiers but no key"))?;
    Ok(Hotkey { modifiers, key })
}

/// Windows virtual-key code for a key name, upper-cased.
fn virtual_key(name: &str) -> Option<u32> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    if let Some(n) = name
        .strip_prefix("NUMPAD")
        .and_then(|n| n.parse::<u32>().ok())
    {
        if n <= 9 {
            return Some(0x60 + n);
        }
    }
    Some(match name {
        "SPACE" => 0x20,
        "ENTER" | "RETURN" => 0x0D,
        "TAB" => 0x09,
        "ESC" | "ESCAPE" => 0x1B,
        "BACKSPACE" => 0x08,
        "INSERT" | "INS" => 0x2D,
        "DELETE" | "DEL" => 0x2E,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" | "PGUP" => 0x21,
        "PAGEDOWN" | "PGDN" => 0x22,
        "LEFT" => 0x25,
        "UP" => 0x26,
        "RIGHT" => 0x27,
        "DOWN" => 0x28,
        "PAUSE" => 0x13,
        "PRINTSCREEN" | "PRTSC" => 0x2C,
        "SCROLLLOCK" => 0x91,
        _ => return None,
    })
}

/// One action's hotkey, ready to register or explaining why it cannot be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The id Windows reports back when the hotkey is pressed.
    pub id: i32,
    pub action: String,
    /// As written in the configuration, for messages.
    pub text: String,
    pub hotkey: Result<Hotkey, String>,
}

/// Every action that has a hotkey, numbered from 1.
pub fn bindings(config: &Config) -> Vec<Binding> {
    config
        .actions
        .iter()
        .filter_map(|a| {
            let text = a.hotkey.as_deref()?.trim();
            (!text.is_empty()).then(|| (a.name.clone(), text.to_string()))
        })
        .enumerate()
        .map(|(i, (action, text))| Binding {
            id: i as i32 + 1,
            hotkey: parse(&text),
            action,
            text,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use switcher_core::actions::Action;
    use switcher_core::config::BackendKind;

    #[test]
    fn the_hotkeys_on_the_desk_parse() {
        assert_eq!(
            parse("CTRL+ALT+1"),
            Ok(Hotkey {
                modifiers: MOD_CONTROL | MOD_ALT,
                key: 0x31
            })
        );
        assert_eq!(
            parse("F24"),
            Ok(Hotkey {
                modifiers: 0,
                key: 0x87
            })
        );
        assert_eq!(
            parse("SHIFT+F24"),
            Ok(Hotkey {
                modifiers: MOD_SHIFT,
                key: 0x87
            })
        );
    }

    #[test]
    fn case_order_and_spacing_do_not_matter() {
        assert_eq!(parse("alt + ctrl + d"), parse("CTRL+ALT+D"));
        assert_eq!(parse("win+f1").unwrap().key, 0x70);
    }

    #[test]
    fn named_keys_are_understood() {
        assert_eq!(parse("CTRL+PAGEDOWN").unwrap().key, 0x22);
        assert_eq!(parse("NUMPAD7").unwrap().key, 0x67);
        assert_eq!(parse("CTRL+SPACE").unwrap().key, 0x20);
    }

    #[test]
    fn nonsense_is_explained_not_guessed() {
        assert!(parse("CTRL+ALT").unwrap_err().contains("no key"));
        assert!(parse("A+B").unwrap_err().contains("two keys"));
        assert!(parse("CTRL+F25").unwrap_err().contains("F25"));
        assert!(parse("CTRL++1").unwrap_err().contains("empty"));
        assert!(parse("HYPER+1").unwrap_err().contains("HYPER"));
    }

    #[test]
    fn only_actions_with_hotkeys_are_bound_and_ids_start_at_one() {
        let mut c = Config::new("laptop", BackendKind::Fake);
        let mut a = Action::new("Toggle center");
        a.hotkey = Some("F24".into());
        let unbound = Action::new("No key");
        let mut b = Action::new("Toggle left");
        b.hotkey = Some("SHIFT+F24".into());
        c.actions = vec![a, unbound, b];

        let found = bindings(&c);
        assert_eq!(found.len(), 2);
        assert_eq!(
            (found[0].id, found[0].action.as_str()),
            (1, "Toggle center")
        );
        assert_eq!((found[1].id, found[1].action.as_str()), (2, "Toggle left"));
        assert!(found.iter().all(|b| b.hotkey.is_ok()));
    }
}
