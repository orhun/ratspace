use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fux_vt::keys::{Direction, Key, KeyPress, Keystroke, Modifiers, encode};

use super::Terminal;

/// Encodes a Crossterm key event using the active VT keyboard mode.
pub fn encode_key(event: KeyEvent, terminal: &Terminal) -> Option<Vec<u8>> {
    if event.kind == KeyEventKind::Release {
        return None;
    }

    let (key, is_back_tab) = key_code(event.code)?;
    let event_modifiers = event.modifiers;
    let press = KeyPress::new(
        key,
        Modifiers {
            ctrl: event_modifiers.contains(KeyModifiers::CONTROL),
            alt: event_modifiers.contains(KeyModifiers::ALT),
            shift: is_back_tab || event_modifiers.contains(KeyModifiers::SHIFT),
        },
    );
    let mut bytes = Vec::new();
    encode::key_bytes(Keystroke::from(press), terminal.key_mode(), &mut bytes);
    Some(bytes)
}

/// Encodes pasted text, including bracketed-paste framing when requested.
pub fn encode_paste(text: &str, terminal: &Terminal) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 12);
    encode::paste(text, terminal.bracketed_paste(), &mut bytes);
    bytes
}

fn key_code(code: KeyCode) -> Option<(Key, bool)> {
    Some(match code {
        KeyCode::Char(character) => (Key::Char(character), false),
        KeyCode::Enter => (Key::Enter, false),
        KeyCode::Tab => (Key::Tab, false),
        KeyCode::BackTab => (Key::Tab, true),
        KeyCode::Backspace => (Key::Backspace, false),
        KeyCode::Esc => (Key::Escape, false),
        KeyCode::Left => (Key::Arrow(Direction::Left), false),
        KeyCode::Right => (Key::Arrow(Direction::Right), false),
        KeyCode::Up => (Key::Arrow(Direction::Up), false),
        KeyCode::Down => (Key::Arrow(Direction::Down), false),
        KeyCode::Home => (Key::Home, false),
        KeyCode::End => (Key::End, false),
        KeyCode::PageUp => (Key::PageUp, false),
        KeyCode::PageDown => (Key::PageDown, false),
        KeyCode::Insert => (Key::Insert, false),
        KeyCode::Delete => (Key::Delete, false),
        KeyCode::F(number) => {
            let key = format!("F{number}").parse::<KeyPress>().ok()?.key;
            (key, false)
        }
        KeyCode::Null => (Key::Char('\0'), false),
        _ => return None,
    })
}
