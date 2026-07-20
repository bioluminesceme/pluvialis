//! Gemini PR over USB serial. The Peregrine speaks this.
//!
//! Six bytes per stroke at 9600 8N1. The top bit of each byte is a framing
//! marker, set on the first byte and clear on the other five, so each byte
//! carries seven bits of key data and 6 x 7 = 42 keys.
//!
//! Full spec in `reference/GEMINI-PR-PROTOCOL.md`. Confirmed against the user's
//! Peregrine on 2026-07-20: `80 00 00 0C 00 00` is `EU` and
//! `80 2C 00 00 00 00` is `SKP`.

use std::io::Read;
use std::time::Duration;

use pluvialis_core::Stroke;

use crate::keymap::Keymap;
use crate::machine::{Machine, MachineError};

pub const PACKET_LEN: usize = 6;

const BAUD: u32 = 9600;

/// How long a read blocks before returning nothing. Short enough that a
/// disconnect is noticed promptly, long enough that idling is nearly free.
const READ_TIMEOUT: Duration = Duration::from_millis(200);

/// The 42 keys, six rows of seven. Index is `byte_index * 7 + bit - 1`.
#[rustfmt::skip]
pub const CHART: [&str; 42] = [
    "Fn",  "#1", "#2", "#3", "#4", "#5",   "#6",
    "S1-", "S2-", "T-", "K-", "P-", "W-",  "H-",
    "R-",  "A-", "O-", "*1", "*2", "res1", "res2",
    "pwr", "*3", "*4", "-E", "-U", "-F",   "-R",
    "-P",  "-B", "-L", "-G", "-T", "-S",   "-D",
    "#7",  "#8", "#9", "#A", "#B", "#C",   "-Z",
];

/// Whether six bytes are correctly framed.
pub fn is_valid_packet(packet: &[u8]) -> bool {
    packet.len() == PACKET_LEN && packet[0] & 0x80 != 0 && packet[1..].iter().all(|b| b & 0x80 == 0)
}

/// Which machine keys a packet reports.
///
/// Note this returns *machine* keys, including `S2-` and `*3`. Collapsing
/// those onto steno keys is the keymap's job, not ours.
pub fn decode_keys(packet: &[u8]) -> Vec<&'static str> {
    let mut keys = Vec::new();
    for (i, byte) in packet.iter().enumerate() {
        // Seven bits per byte, high to low, skipping the framing bit.
        for j in 1..8 {
            if byte & (0x80 >> j) != 0 {
                keys.push(CHART[i * 7 + j - 1]);
            }
        }
    }
    keys
}

pub struct GeminiPr {
    keymap: Keymap,
    port: Option<Box<dyn serialport::SerialPort>>,
    /// Bytes received but not yet forming a whole packet.
    buffer: Vec<u8>,
    /// The port that worked last time, tried first on the next scan so a
    /// reconnect does not re-sniff every port on the system.
    preferred: Option<String>,
}

impl Default for GeminiPr {
    fn default() -> Self {
        Self::new()
    }
}

impl GeminiPr {
    pub fn new() -> Self {
        GeminiPr {
            keymap: Keymap::gemini_pr(),
            port: None,
            buffer: Vec::new(),
            preferred: None,
        }
    }

    /// Candidate ports, most likely first.
    ///
    /// There is no way to know a COM port is a steno keyboard without listening
    /// to it, so ordering matters: the port that worked last, then USB serial
    /// devices, then everything else.
    fn candidates() -> Vec<String> {
        let ports = match serialport::available_ports() {
            Ok(ports) => ports,
            Err(e) => {
                log::debug!("could not enumerate serial ports: {e}");
                return Vec::new();
            }
        };

        let (usb, other): (Vec<_>, Vec<_>) = ports
            .into_iter()
            .partition(|p| matches!(p.port_type, serialport::SerialPortType::UsbPort(_)));

        usb.into_iter().chain(other).map(|p| p.port_name).collect()
    }

    /// Turn buffered bytes into strokes, resynchronising if framing is lost.
    fn drain_buffer(&mut self) -> Vec<Stroke> {
        let mut strokes = Vec::new();

        loop {
            // A packet must start on a byte with the high bit set. If it does
            // not we are out of frame, so skip forward to one that does rather
            // than decoding garbage into plausible-looking strokes.
            match self.buffer.iter().position(|b| b & 0x80 != 0) {
                Some(0) => {}
                Some(skip) => {
                    log::warn!("out of frame, discarding {skip} bytes");
                    self.buffer.drain(..skip);
                }
                None => {
                    if !self.buffer.is_empty() {
                        log::warn!("out of frame, discarding {} bytes", self.buffer.len());
                        self.buffer.clear();
                    }
                    break;
                }
            }

            if self.buffer.len() < PACKET_LEN {
                break;
            }

            let packet: Vec<u8> = self.buffer.drain(..PACKET_LEN).collect();
            if !is_valid_packet(&packet) {
                // A high bit inside the packet body means the real frame starts
                // later. Drop only the first byte and try again from there.
                log::warn!("malformed packet {packet:02X?}, resynchronising");
                self.buffer.splice(0..0, packet[1..].iter().copied());
                continue;
            }

            let keys = decode_keys(&packet);
            match self.keymap.stroke(&keys) {
                Ok(Some(stroke)) => strokes.push(stroke),
                // A chord of only unmapped keys, such as Fn alone.
                Ok(None) => log::debug!("chord with no bound keys: {keys:?}"),
                Err(e) => log::warn!("chord {keys:?} is not a valid stroke: {e}"),
            }
        }

        strokes
    }
}

impl Machine for GeminiPr {
    fn name(&self) -> &'static str {
        "Gemini PR"
    }

    fn connect(&mut self) -> Result<String, MachineError> {
        let mut candidates = GeminiPr::candidates();
        if let Some(preferred) = &self.preferred
            && let Some(at) = candidates.iter().position(|p| p == preferred)
        {
            candidates.swap(0, at);
        }

        if candidates.is_empty() {
            return Err(MachineError::NotAttached);
        }

        for name in candidates {
            match serialport::new(&name, BAUD)
                .data_bits(serialport::DataBits::Eight)
                .parity(serialport::Parity::None)
                .stop_bits(serialport::StopBits::One)
                .flow_control(serialport::FlowControl::None)
                .timeout(READ_TIMEOUT)
                .open()
            {
                Ok(port) => {
                    log::info!("opened {name} at {BAUD} baud");
                    self.port = Some(port);
                    self.buffer.clear();
                    self.preferred = Some(name.clone());
                    return Ok(name);
                }
                // A port that is busy or not ours is ordinary, not a problem
                // worth reporting: other software may legitimately hold it.
                Err(e) => log::debug!("{name} not usable: {e}"),
            }
        }

        Err(MachineError::NotAttached)
    }

    fn poll(&mut self) -> Result<Vec<Stroke>, MachineError> {
        let Some(port) = self.port.as_mut() else {
            return Err(MachineError::NotAttached);
        };

        let mut chunk = [0u8; 64];
        match port.read(&mut chunk) {
            Ok(0) => {}
            Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
            // A timeout means the user simply is not writing. This is the
            // common case and must not be treated as the device going away.
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(MachineError::Io(e.to_string())),
        }

        Ok(self.drain_buffer())
    }

    fn disconnect(&mut self) {
        // Drop closes the handle. Order matters: take the port out, then let it
        // fall, so there is never a window where we hold a stale handle.
        self.port.take();
        self.buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from the user's Peregrine on 2026-07-20.
    const REAL_EU: [u8; 6] = [0x80, 0x00, 0x00, 0x0C, 0x00, 0x00];
    const REAL_SKP: [u8; 6] = [0x80, 0x2C, 0x00, 0x00, 0x00, 0x00];

    fn stroke_of(packet: &[u8]) -> String {
        let keys = decode_keys(packet);
        let stroke = Keymap::gemini_pr().stroke(&keys).unwrap().unwrap();
        Stroke::render_outline(&[stroke])
    }

    #[test]
    fn the_chart_has_one_entry_per_key() {
        assert_eq!(CHART.len(), 42);
    }

    #[test]
    fn real_hardware_packet_decodes_to_eu() {
        assert!(is_valid_packet(&REAL_EU));
        assert_eq!(decode_keys(&REAL_EU), vec!["-E", "-U"]);
        assert_eq!(stroke_of(&REAL_EU), "EU");
    }

    /// The one that proves the keymap layer earns its place: the split S
    /// reports as S2-, and only the keymap knows that is an S.
    #[test]
    fn real_hardware_packet_decodes_to_skp() {
        assert!(is_valid_packet(&REAL_SKP));
        assert_eq!(decode_keys(&REAL_SKP), vec!["S2-", "K-", "P-"]);
        assert_eq!(stroke_of(&REAL_SKP), "SKP");
    }

    #[test]
    fn framing_requires_the_high_bit_on_the_first_byte_only() {
        assert!(!is_valid_packet(&[0x00, 0, 0, 0, 0, 0]), "no marker");
        assert!(!is_valid_packet(&[0x80, 0x80, 0, 0, 0, 0]), "marker inside");
        assert!(!is_valid_packet(&[0x80, 0, 0, 0, 0]), "too short");
        assert!(is_valid_packet(&REAL_SKP));
    }

    /// Bit order is high to low within each byte. Reversing it yields strokes
    /// that look reasonable and are wrong, so pin both ends of a row.
    #[test]
    fn bit_order_runs_high_to_low() {
        // Byte 1, bit 1 (0x40) is the first key of the second row.
        assert_eq!(decode_keys(&[0x80, 0x40, 0, 0, 0, 0]), vec!["S1-"]);
        // Byte 1, bit 7 (0x01) is the last key of that row.
        assert_eq!(decode_keys(&[0x80, 0x01, 0, 0, 0, 0]), vec!["H-"]);
    }

    #[test]
    fn every_chart_position_is_reachable() {
        for (index, key) in CHART.iter().enumerate() {
            let (byte, bit) = (index / 7, index % 7 + 1);
            let mut packet = [0u8; 6];
            packet[0] = 0x80;
            packet[byte] |= 0x80 >> bit;
            assert!(
                decode_keys(&packet).contains(key),
                "chart index {index} ({key}) is unreachable"
            );
        }
    }

    #[test]
    fn a_full_chord_decodes_every_pressed_key() {
        // S1- T- K- P- across byte 1: bits 1, 3, 4, 5.
        let packet = [0x80, 0x40 | 0x10 | 0x08 | 0x04, 0, 0, 0, 0];
        assert_eq!(decode_keys(&packet), vec!["S1-", "T-", "K-", "P-"]);
    }

    #[test]
    fn buffered_bytes_become_strokes() {
        let mut machine = GeminiPr::new();
        machine.buffer.extend_from_slice(&REAL_EU);
        machine.buffer.extend_from_slice(&REAL_SKP);

        let strokes = machine.drain_buffer();
        assert_eq!(strokes.len(), 2);
        assert_eq!(Stroke::render_outline(&strokes), "EU/SKP");
        assert!(machine.buffer.is_empty());
    }

    #[test]
    fn a_partial_packet_waits_for_the_rest() {
        let mut machine = GeminiPr::new();
        machine.buffer.extend_from_slice(&REAL_SKP[..4]);
        assert!(machine.drain_buffer().is_empty());

        machine.buffer.extend_from_slice(&REAL_SKP[4..]);
        let strokes = machine.drain_buffer();
        assert_eq!(Stroke::render_outline(&strokes), "SKP");
    }

    #[test]
    fn leading_junk_is_skipped_to_reach_the_next_frame() {
        let mut machine = GeminiPr::new();
        machine.buffer.extend_from_slice(&[0x01, 0x02, 0x03]);
        machine.buffer.extend_from_slice(&REAL_SKP);

        let strokes = machine.drain_buffer();
        assert_eq!(Stroke::render_outline(&strokes), "SKP");
        assert!(machine.buffer.is_empty());
    }

    /// A marker byte inside the body means the real frame starts later. We must
    /// recover the following stroke rather than losing it with the bad packet.
    #[test]
    fn a_misframed_packet_resynchronises_without_losing_the_next_stroke() {
        let mut machine = GeminiPr::new();
        machine.buffer.push(0x80);
        machine.buffer.extend_from_slice(&REAL_SKP);

        let strokes = machine.drain_buffer();
        assert_eq!(Stroke::render_outline(&strokes), "SKP");
    }

    #[test]
    fn a_chord_of_only_unmapped_keys_yields_no_stroke() {
        let mut machine = GeminiPr::new();
        // Byte 0 bit 1 is Fn, which is unbound.
        machine.buffer.extend_from_slice(&[0xC0, 0, 0, 0, 0, 0]);
        assert!(machine.drain_buffer().is_empty());
    }

    #[test]
    fn disconnecting_when_not_connected_is_harmless() {
        let mut machine = GeminiPr::new();
        machine.disconnect();
        machine.disconnect();
        assert!(machine.port.is_none());
    }

    #[test]
    fn polling_without_a_port_reports_absence_not_failure() {
        let mut machine = GeminiPr::new();
        let error = machine.poll().unwrap_err();
        assert!(error.is_absence());
    }
}
