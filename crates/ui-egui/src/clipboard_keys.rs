//! Browser clipboard events supersede their matching command key events.

pub(crate) fn drop_duplicate_clipboard_keys(events: &mut Vec<egui::Event>, web: bool) {
    if !web {
        return;
    }
    // Shift+C/V have formatting bindings; Alt+C/X are separate shortcuts.
    // Alt+V's paste event dispatches Paste Special using the raw modifiers.
    events.retain(|event| {
        !matches!(event, egui::Event::Key { key: key @ (egui::Key::C | egui::Key::X | egui::Key::V), modifiers, .. }
            if modifiers.command && !modifiers.shift && (!modifiers.alt || *key == egui::Key::V))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Key, Modifiers};

    fn key(key: Key, modifiers: Modifiers, pressed: bool) -> Event {
        Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }
    }

    #[test]
    fn web_clipboard_events_replace_command_key_press_and_release() {
        for modifiers in [Modifiers::CTRL | Modifiers::COMMAND, Modifiers::MAC_CMD | Modifiers::COMMAND] {
            for (letter, clipboard) in [(Key::V, Event::Paste("x".into())), (Key::C, Event::Copy), (Key::X, Event::Cut)] {
                let mut events = vec![key(letter, modifiers, true), key(letter, modifiers, false), clipboard.clone()];
                drop_duplicate_clipboard_keys(&mut events, true);
                assert_eq!(events, vec![clipboard]);
            }
        }
    }

    #[test]
    fn web_paste_special_keeps_only_the_paste_event() {
        let modifiers = Modifiers::CTRL | Modifiers::COMMAND | Modifiers::ALT;
        let mut events = vec![key(Key::V, modifiers, true), key(Key::V, modifiers, false), Event::Paste("x".into())];
        drop_duplicate_clipboard_keys(&mut events, true);
        assert_eq!(events, vec![Event::Paste("x".into())]);
    }

    #[test]
    fn web_keeps_shift_and_other_alt_bindings() {
        let mut events = Vec::new();
        for letter in [Key::C, Key::X, Key::V] {
            for alt in [false, true] {
                let modifiers = Modifiers { command: true, ctrl: true, shift: true, alt, ..Default::default() };
                events.extend([key(letter, modifiers, true), key(letter, modifiers, false)]);
            }
        }
        for letter in [Key::C, Key::X] {
            let modifiers = Modifiers::CTRL | Modifiers::COMMAND | Modifiers::ALT;
            events.extend([key(letter, modifiers, true), key(letter, modifiers, false)]);
        }
        let original = events.clone();
        drop_duplicate_clipboard_keys(&mut events, true);
        assert_eq!(events, original);
    }

    #[test]
    fn web_keeps_unrelated_input() {
        let mut events = vec![
            key(Key::Z, Modifiers::CTRL | Modifiers::COMMAND, true),
            Event::Text("v".into()),
            Event::PointerMoved(egui::pos2(1.0, 2.0)),
        ];
        for letter in [Key::C, Key::X, Key::V] {
            for modifiers in [Modifiers::NONE, Modifiers::ALT, Modifiers::ALT | Modifiers::SHIFT] {
                events.extend([key(letter, modifiers, true), key(letter, modifiers, false)]);
            }
        }
        let original = events.clone();
        drop_duplicate_clipboard_keys(&mut events, true);
        assert_eq!(events, original);
    }

    #[test]
    fn desktop_keeps_all_clipboard_keys_and_events() {
        let mut events = Vec::new();
        for letter in [Key::C, Key::X, Key::V] {
            for alt in [false, true] {
                for shift in [false, true] {
                    let modifiers = Modifiers { command: true, ctrl: true, alt, shift, ..Default::default() };
                    events.extend([key(letter, modifiers, true), key(letter, modifiers, false)]);
                }
            }
        }
        events.extend([Event::Copy, Event::Cut, Event::Paste("x".into())]);
        let original = events.clone();
        drop_duplicate_clipboard_keys(&mut events, false);
        assert_eq!(events, original);
    }
}
