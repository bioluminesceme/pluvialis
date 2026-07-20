//! Stenograph USB, the protocol the user's Luminex CSE speaks.
//!
//! Three parts: [`packet`] is the wire format and is portable, `transport` is
//! the Windows device handle, and this module is the read loop that turns one
//! into the other.
//!
//! The read loop is where Plover's plugin fails the user, so read
//! `thingstonote.md` before changing it. Absent hardware, an unopened realtime
//! file, and a closed file are all *states* here, never failures.

pub mod packet;

#[cfg(windows)]
mod transport;
#[cfg(windows)]
mod writer;

#[cfg(windows)]
pub use writer::Stenograph;

/// A short label for the status bar, pulled out of a device path.
///
/// Paths look like `\\?\usb#vid_112b&pid_000d&mi_00#6&182e212a&0&0000#{guid}`,
/// which is too long to show. The second segment identifies the hardware and
/// is the useful part.
pub fn short_device_label(path: &str) -> String {
    path.split('#')
        .nth(1)
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.to_uppercase())
        .unwrap_or_else(|| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real path shape, taken from this machine's registry entry.
    #[test]
    fn a_device_path_shortens_to_its_hardware_id() {
        let path = r"\\?\usb#vid_112b&pid_000d&mi_00#6&182e212a&0&0000#{202e68c5-5980-4a60-b761-77c4de9d5dbf}";
        assert_eq!(short_device_label(path), "VID_112B&PID_000D&MI_00");
    }

    #[test]
    fn an_unexpected_path_shape_is_shown_as_is_rather_than_lost() {
        assert_eq!(short_device_label("something-else"), "something-else");
        assert_eq!(short_device_label(""), "");
    }
}
