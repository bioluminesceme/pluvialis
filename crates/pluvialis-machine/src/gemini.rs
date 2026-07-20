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

/// USB vendor IDs that belong to other steno protocols.
///
/// Confirmed on 2026-07-20: the user's Luminex enumerates as
/// `VID_112B&PID_000D` and offers a "Stenograph Writer Serial Port" alongside
/// its real protocol interface. That port is silent, so opening it here yields
/// a connection that looks healthy and never produces a stroke. Worse, holding
/// it open would block the Stenograph implementation from the same device.
///
/// A machine listed here is not unsupported. It is supported *elsewhere*.
const OTHER_PROTOCOL_VIDS: [u16; 1] = [
    0x112B, // Stenograph (Luminex CSE and relatives), handled by the Stenograph machine
];

/// USB vendor IDs known to speak Gemini PR, tried before unknown ports.
const LIKELY_GEMINI_VIDS: [u16; 1] = [
    0xFEED, // QMK default, which is what the user's Peregrine reports
];

/// Order candidate ports by how likely they are to be a Gemini PR keyboard,
/// dropping any that belong to another protocol.
///
/// Split out from enumeration so the ranking is testable without hardware.
fn rank_candidates(ports: Vec<serialport::SerialPortInfo>, preferred: Option<&str>) -> Vec<String> {
    let rank = |port: &serialport::SerialPortInfo| -> Option<u8> {
        // Exclusion is checked before preference on purpose. A remembered port
        // must not be able to revive an excluded device: the machine on the
        // other end can change while the port name stays the same.
        let base = match &port.port_type {
            serialport::SerialPortType::UsbPort(usb) => {
                if OTHER_PROTOCOL_VIDS.contains(&usb.vid) {
                    return None;
                } else if LIKELY_GEMINI_VIDS.contains(&usb.vid) {
                    1
                } else {
                    2
                }
            }
            // Not USB, so not a modern steno keyboard, but not impossible.
            _ => 3,
        };

        if Some(port.port_name.as_str()) == preferred {
            Some(0)
        } else {
            Some(base)
        }
    };

    let mut ranked: Vec<(u8, String)> = ports
        .into_iter()
        .filter_map(|port| rank(&port).map(|r| (r, port.port_name)))
        .collect();

    // Stable so ports of equal rank keep enumeration order.
    ranked.sort_by_key(|(r, _)| *r);
    ranked.into_iter().map(|(_, name)| name).collect()
}

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
    /// to it, so ordering and exclusion both matter.
    fn candidates(preferred: Option<&str>) -> Vec<String> {
        let ports = match serialport::available_ports() {
            Ok(ports) => ports,
            Err(e) => {
                log::debug!("could not enumerate serial ports: {e}");
                return Vec::new();
            }
        };
        rank_candidates(ports, preferred)
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
        let candidates = GeminiPr::candidates(self.preferred.as_deref());

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
                Ok(mut port) => {
                    // Assert DTR and RTS. A USB CDC device commonly treats DTR
                    // as "a host is listening" and stays silent without it, so
                    // the port opens, looks healthy, and delivers nothing. That
                    // failure mode is indistinguishable from the user simply
                    // not writing, which makes it expensive to diagnose.
                    if let Err(e) = port.write_data_terminal_ready(true) {
                        log::debug!("{name}: could not assert DTR: {e}");
                    }
                    if let Err(e) = port.write_request_to_send(true) {
                        log::debug!("{name}: could not assert RTS: {e}");
                    }

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

    fn usb_port(name: &str, vid: u16, pid: u16) -> serialport::SerialPortInfo {
        serialport::SerialPortInfo {
            port_name: name.to_owned(),
            port_type: serialport::SerialPortType::UsbPort(serialport::UsbPortInfo {
                vid,
                pid,
                serial_number: None,
                manufacturer: None,
                product: None,
            }),
        }
    }

    /// The user's actual hardware, as enumerated on 2026-07-20.
    fn peregrine() -> serialport::SerialPortInfo {
        usb_port("COM11", 0xFEED, 0x6060)
    }
    fn luminex_serial() -> serialport::SerialPortInfo {
        usb_port("COM3", 0x112B, 0x000D)
    }

    /// The bug this fixes: with both machines attached, the Gemini scanner
    /// would open the Luminex's serial port, report a healthy connection, and
    /// never receive a stroke. It would also block the Stenograph
    /// implementation from the same device.
    #[test]
    fn the_luminex_serial_port_is_never_a_gemini_candidate() {
        let ranked = rank_candidates(vec![luminex_serial(), peregrine()], None);
        assert_eq!(ranked, vec!["COM11"], "the Luminex must not be offered");
    }

    #[test]
    fn the_luminex_is_excluded_even_when_it_is_the_only_port() {
        let ranked = rank_candidates(vec![luminex_serial()], None);
        assert!(
            ranked.is_empty(),
            "no Gemini keyboard is attached, so there is nothing to try"
        );
    }

    #[test]
    fn a_qmk_keyboard_outranks_an_unknown_usb_serial_device() {
        let ranked = rank_candidates(vec![usb_port("COM9", 0x1234, 0x0001), peregrine()], None);
        assert_eq!(ranked, vec!["COM11", "COM9"]);
    }

    #[test]
    fn the_last_working_port_is_tried_first() {
        let ranked = rank_candidates(
            vec![peregrine(), usb_port("COM9", 0x1234, 0x0001)],
            Some("COM9"),
        );
        assert_eq!(ranked, vec!["COM9", "COM11"]);
    }

    /// A remembered port that belongs to another protocol must not be revived
    /// by the preference, or the bug returns through the back door. This is
    /// reachable in practice: the Peregrine could be unplugged and the Luminex
    /// take the same COM number.
    #[test]
    fn preferring_a_port_does_not_override_the_exclusion() {
        let ranked = rank_candidates(vec![luminex_serial(), peregrine()], Some("COM3"));
        assert_eq!(
            ranked,
            vec!["COM11"],
            "an excluded port must stay excluded even when remembered"
        );
    }

    #[test]
    fn ports_of_equal_rank_keep_enumeration_order() {
        let ranked = rank_candidates(
            vec![usb_port("COM5", 0x1111, 1), usb_port("COM6", 0x2222, 2)],
            None,
        );
        assert_eq!(ranked, vec!["COM5", "COM6"]);
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
