use crossterm::event::KeyCode;
use smithay::input::keyboard::{Keysym, keysyms};

pub(super) fn crossterm_key(sym: Keysym) -> Option<KeyCode> {
    let raw = sym.raw();
    Some(match raw {
        keysyms::KEY_BackSpace => KeyCode::Backspace,
        keysyms::KEY_Tab => KeyCode::Tab,
        keysyms::KEY_Return => KeyCode::Enter,
        keysyms::KEY_Escape => KeyCode::Esc,
        keysyms::KEY_Home => KeyCode::Home,
        keysyms::KEY_Left => KeyCode::Left,
        keysyms::KEY_Up => KeyCode::Up,
        keysyms::KEY_Right => KeyCode::Right,
        keysyms::KEY_Down => KeyCode::Down,
        keysyms::KEY_Page_Up => KeyCode::PageUp,
        keysyms::KEY_Page_Down => KeyCode::PageDown,
        keysyms::KEY_End => KeyCode::End,
        keysyms::KEY_Insert => KeyCode::Insert,
        keysyms::KEY_Delete => KeyCode::Delete,
        keysyms::KEY_F1..=keysyms::KEY_F35 => KeyCode::F((raw - keysyms::KEY_F1 + 1) as u8),
        _ => KeyCode::Char(sym.key_char()?),
    })
}
