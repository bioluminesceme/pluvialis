//! Emitting keystrokes with `SendInput`.

use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY,
};

use crate::OutputError;
use crate::keys::{Chord, Key};

/// Backspace, for deleting what a correction replaces.
const VK_BACK: u16 = 0x08;

/// Sends keystrokes to whichever window has focus.
#[derive(Debug, Default)]
pub struct Keyboard {
    _private: (),
}

fn key_event(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn press(key: Key, up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if key.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    key_event(key.vk, 0, flags)
}

/// One UTF-16 code unit as a synthesised character.
///
/// `KEYEVENTF_UNICODE` sends the character itself rather than a physical key,
/// so output does not depend on the user's keyboard layout. Driving this with
/// virtual keys instead would produce different characters on a Dutch layout
/// than a US one.
fn unicode(unit: u16, up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    key_event(0, unit, flags)
}

impl Keyboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Send a batch as one call.
    ///
    /// One `SendInput` for the whole batch, never one per key: Windows keeps a
    /// single call's events contiguous, so nothing the user types by hand can
    /// interleave into the middle of a word.
    fn send(&self, events: &[INPUT]) -> Result<(), OutputError> {
        if events.is_empty() {
            return Ok(());
        }
        let sent = unsafe { SendInput(events, size_of::<INPUT>() as i32) };
        if sent as usize != events.len() {
            return Err(OutputError::Partial {
                sent: sent as usize,
                expected: events.len(),
            });
        }
        Ok(())
    }

    /// Delete `count` characters, then type `text`.
    ///
    /// `count` is keypresses, not bytes: one backspace deletes one character.
    pub fn send_edit(&self, count: usize, text: &str) -> Result<(), OutputError> {
        let mut events = Vec::with_capacity(count * 2 + text.len() * 2);

        for _ in 0..count {
            events.push(key_event(VK_BACK, 0, KEYBD_EVENT_FLAGS(0)));
            events.push(key_event(VK_BACK, 0, KEYEVENTF_KEYUP));
        }

        // encode_utf16 splits astral characters into surrogate pairs, and each
        // half is sent as its own event. Windows reassembles them.
        for unit in text.encode_utf16() {
            events.push(unicode(unit, false));
            events.push(unicode(unit, true));
        }

        self.send(&events)
    }

    /// Perform a parsed key combo.
    pub fn send_combo(&self, chords: &[Chord]) -> Result<(), OutputError> {
        let mut events = Vec::new();

        for chord in chords {
            for modifier in &chord.modifiers {
                events.push(press(*modifier, false));
            }
            events.push(press(chord.key, false));
            events.push(press(chord.key, true));
            // Release in reverse, so the modifier stack unwinds the way it was
            // built.
            for modifier in chord.modifiers.iter().rev() {
                events.push(press(*modifier, true));
            }
        }

        self.send(&events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Building the event list is testable; actually sending it would type into
    /// whatever window happens to be focused, including the test runner's.
    #[test]
    fn an_empty_batch_sends_nothing() {
        let keyboard = Keyboard::new();
        assert!(keyboard.send(&[]).is_ok());
        assert!(keyboard.send_edit(0, "").is_ok());
        assert!(keyboard.send_combo(&[]).is_ok());
    }
}
