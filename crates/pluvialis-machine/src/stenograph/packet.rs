//! The Stenograph wire format: a 32 byte header, an optional payload, and the
//! chord encoding inside a read response.
//!
//! Deliberately free of any Windows API. The format is the half of this
//! protocol that can be tested without a writer on the desk, so it lives apart
//! from the transport and carries the tests.

use crate::machine::MachineError;

/// Every packet opens with these two bytes.
const SYNC: [u8; 2] = *b"SG";

/// Header layout, little endian, no padding (the Python's `<2sIH6I`):
/// sync, sequence, packet type, data length, then five parameters.
pub const HEADER_LEN: usize = 32;

/// Bytes to request per read.
pub const MAX_READ: u32 = 0x200;

/// One chord on the wire: four steno bytes, then four timestamp bytes we drop.
pub const CHORD_LEN: usize = 8;

pub const PACKET_ERROR: u16 = 0x06;
pub const PACKET_OPEN_FILE: u16 = 0x11;
pub const PACKET_READ_FILE: u16 = 0x13;

pub const ERROR_UNABLE_TO_PERFORM: u32 = 3;
pub const ERROR_FILE_NOT_AVAILABLE: u32 = 7;
/// The user has not started writing yet. Routine, not a failure.
pub const ERROR_NO_REALTIME_FILE: u32 = 8;
/// The file was closed. Routine: reopen and carry on.
pub const ERROR_FINISHED_READING_CLOSED_FILE: u32 = 9;

/// The file the writer appends to as the user writes.
const REALTIME_FILE: &[u8] = b"REALTIME.000";

/// Disk id, ASCII 'A'.
const DISK_A: u32 = 0x41;

/// A decoded response header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub sequence: u32,
    pub packet_type: u16,
    pub data_length: u32,
    /// p1 through p5. Only p1 is read (it carries the error code), but keeping
    /// all five makes a protocol log worth reading.
    pub params: [u32; 5],
}

impl Response {
    pub fn is_error(&self) -> bool {
        self.packet_type == PACKET_ERROR
    }

    /// Meaningful only when [`Self::is_error`].
    pub fn error_code(&self) -> u32 {
        self.params[0]
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

/// Build a request. Payloads are zero padded to a multiple of 8, and
/// `data_length` counts the padding, matching the writer's expectation.
fn encode(sequence: u32, packet_type: u16, params: [u32; 5], data: &[u8]) -> Vec<u8> {
    let padded_len = data.len().next_multiple_of(8);

    let mut packet = Vec::with_capacity(HEADER_LEN + padded_len);
    packet.extend_from_slice(&SYNC);
    packet.extend_from_slice(&sequence.to_le_bytes());
    packet.extend_from_slice(&packet_type.to_le_bytes());
    packet.extend_from_slice(&(padded_len as u32).to_le_bytes());
    for param in params {
        packet.extend_from_slice(&param.to_le_bytes());
    }
    packet.extend_from_slice(data);
    packet.resize(HEADER_LEN + padded_len, 0);
    packet
}

/// Ask the writer to open the realtime file.
pub fn open_file_request(sequence: u32) -> Vec<u8> {
    encode(
        sequence,
        PACKET_OPEN_FILE,
        [DISK_A, 0, 0, 0, 0],
        REALTIME_FILE,
    )
}

/// Ask for up to [`MAX_READ`] bytes starting at `offset`.
pub fn read_file_request(sequence: u32, offset: u32) -> Vec<u8> {
    encode(sequence, PACKET_READ_FILE, [offset, MAX_READ, 0, 0, 0], &[])
}

pub fn decode_header(bytes: &[u8]) -> Result<Response, MachineError> {
    if bytes.len() < HEADER_LEN {
        return Err(MachineError::Protocol(format!(
            "short header: {} bytes, expected {HEADER_LEN}",
            bytes.len()
        )));
    }
    if bytes[0..2] != SYNC {
        return Err(MachineError::Protocol(format!(
            "bad sync {:02x?}, expected {SYNC:02x?}",
            &bytes[0..2]
        )));
    }

    let mut params = [0u32; 5];
    for (index, param) in params.iter_mut().enumerate() {
        *param = u32_at(bytes, 12 + index * 4);
    }

    Ok(Response {
        sequence: u32_at(bytes, 2),
        packet_type: u16::from_le_bytes([bytes[6], bytes[7]]),
        data_length: u32_at(bytes, 8),
        params,
    })
}

/// The writer's key chart. Four rows of six, one row per steno byte.
///
/// `^` is a machine key with no steno meaning; the keymap drops it. It is
/// unrelated to the `^` used for attachment in dictionary values.
pub const CHART: [[&str; 6]; 4] = [
    ["^", "#", "S-", "T-", "K-", "P-"],
    ["W-", "H-", "R-", "A-", "O-", "*"],
    ["-E", "-U", "-F", "-R", "-P", "-B"],
    ["-L", "-G", "-T", "-S", "-D", "-Z"],
];

/// Machine keys in one chord.
///
/// Only the first four bytes are read; the trailing four are a timestamp we do
/// not use. Within a byte the top two bits are framing and always set, and the
/// low six are keys running high to low: bit 5 is the first key in the row,
/// bit 0 the last. It is `1 << (5 - index)`, not `1 << index`. Reversing it
/// yields strokes that are wrong and look entirely plausible.
pub fn decode_chord(chord: &[u8]) -> Result<Vec<&'static str>, MachineError> {
    let mut keys = Vec::new();

    for (row, &byte) in CHART.iter().zip(chord.iter()) {
        if byte < 0b1100_0000 {
            return Err(MachineError::Protocol(format!(
                "steno byte {byte:#04x} has its framing bits clear, the stream is misaligned"
            )));
        }
        for (index, key) in row.iter().enumerate() {
            if byte & (1 << (5 - index)) != 0 {
                keys.push(*key);
            }
        }
    }

    Ok(keys)
}

/// Split a read payload into chords and decode each.
pub fn decode_chords(data: &[u8]) -> Result<Vec<Vec<&'static str>>, MachineError> {
    if !data.len().is_multiple_of(CHORD_LEN) {
        return Err(MachineError::Protocol(format!(
            "payload of {} bytes is not a whole number of {CHORD_LEN} byte chords",
            data.len()
        )));
    }
    data.chunks(CHORD_LEN).map(decode_chord).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the four steno bytes for a set of chart positions.
    fn chord(positions: &[(usize, usize)]) -> Vec<u8> {
        let mut bytes = [0b1100_0000u8; 8];
        for &(row, index) in positions {
            bytes[row] |= 1 << (5 - index);
        }
        bytes.to_vec()
    }

    #[test]
    fn a_request_header_starts_with_the_sync_bytes() {
        let packet = read_file_request(7, 0);
        assert_eq!(&packet[0..2], b"SG");
    }

    #[test]
    fn a_read_request_carries_the_offset_and_the_read_size() {
        let packet = read_file_request(3, 0x40);
        let decoded = decode_header(&packet).unwrap();

        assert_eq!(decoded.sequence, 3);
        assert_eq!(decoded.packet_type, PACKET_READ_FILE);
        assert_eq!(decoded.params[0], 0x40, "offset");
        assert_eq!(decoded.params[1], MAX_READ, "byte count");
        assert_eq!(decoded.data_length, 0);
        assert_eq!(packet.len(), HEADER_LEN);
    }

    /// `data_length` counts the padding, so a 12 byte name reports 16.
    #[test]
    fn an_open_request_pads_the_filename_and_counts_the_padding() {
        let packet = open_file_request(1);
        let decoded = decode_header(&packet).unwrap();

        assert_eq!(decoded.packet_type, PACKET_OPEN_FILE);
        assert_eq!(decoded.params[0], DISK_A);
        assert_eq!(REALTIME_FILE.len(), 12, "the name itself is 12 bytes");
        assert_eq!(decoded.data_length, 16, "padded to a multiple of 8");
        assert_eq!(packet.len(), HEADER_LEN + 16);
        assert_eq!(&packet[HEADER_LEN..HEADER_LEN + 12], REALTIME_FILE);
        assert_eq!(&packet[HEADER_LEN + 12..], &[0, 0, 0, 0], "zero padding");
    }

    #[test]
    fn a_header_without_the_sync_bytes_is_a_protocol_error() {
        let mut packet = read_file_request(1, 0);
        packet[0] = b'X';
        assert!(matches!(
            decode_header(&packet),
            Err(MachineError::Protocol(_))
        ));
    }

    #[test]
    fn a_truncated_header_is_a_protocol_error_rather_than_a_panic() {
        assert!(matches!(
            decode_header(&[b'S', b'G', 0, 0]),
            Err(MachineError::Protocol(_))
        ));
    }

    #[test]
    fn an_error_response_exposes_its_code() {
        let packet = encode(9, PACKET_ERROR, [ERROR_NO_REALTIME_FILE, 0, 0, 0, 0], &[]);
        let decoded = decode_header(&packet).unwrap();

        assert!(decoded.is_error());
        assert_eq!(decoded.error_code(), ERROR_NO_REALTIME_FILE);
    }

    #[test]
    fn an_empty_chord_yields_no_keys() {
        assert_eq!(decode_chord(&chord(&[])).unwrap(), Vec::<&str>::new());
    }

    /// Bit 5 is the first key in the row and bit 0 the last. If this reverses,
    /// every stroke silently becomes a different valid looking stroke.
    #[test]
    fn the_first_key_in_a_row_is_the_high_bit() {
        let keys = decode_chord(&chord(&[(0, 0)])).unwrap();
        assert_eq!(keys, vec!["^"], "bit 5 of byte 0 is the first chart entry");

        let keys = decode_chord(&chord(&[(0, 5)])).unwrap();
        assert_eq!(keys, vec!["P-"], "bit 0 of byte 0 is the last chart entry");
    }

    #[test]
    fn every_chart_position_decodes_to_its_own_key() {
        for (row_index, row) in CHART.iter().enumerate() {
            for (key_index, key) in row.iter().enumerate() {
                let keys = decode_chord(&chord(&[(row_index, key_index)])).unwrap();
                assert_eq!(keys, vec![*key], "row {row_index} index {key_index}");
            }
        }
    }

    #[test]
    fn a_multi_key_chord_decodes_in_chart_order() {
        // K- and A- and -T, which is the outline KAT.
        let keys = decode_chord(&chord(&[(0, 4), (1, 3), (3, 2)])).unwrap();
        assert_eq!(keys, vec!["K-", "A-", "-T"]);
    }

    #[test]
    fn the_timestamp_bytes_are_ignored() {
        let mut bytes = chord(&[(0, 4), (1, 3), (3, 2)]);
        bytes[4..8].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(decode_chord(&bytes).unwrap(), vec!["K-", "A-", "-T"]);
    }

    /// Misalignment must surface loudly rather than produce plausible strokes.
    #[test]
    fn a_steno_byte_without_its_framing_bits_is_a_protocol_error() {
        let mut bytes = chord(&[(0, 4)]);
        bytes[1] = 0b0010_0000;
        assert!(matches!(
            decode_chord(&bytes),
            Err(MachineError::Protocol(_))
        ));
    }

    #[test]
    fn a_payload_splits_into_chords() {
        let mut data = chord(&[(0, 4), (1, 3), (3, 2)]);
        data.extend_from_slice(&chord(&[(1, 5)]));

        let chords = decode_chords(&data).unwrap();
        assert_eq!(chords.len(), 2);
        assert_eq!(chords[0], vec!["K-", "A-", "-T"]);
        assert_eq!(chords[1], vec!["*"]);
    }

    #[test]
    fn a_partial_chord_is_a_protocol_error() {
        assert!(matches!(
            decode_chords(&[0xFF; 6]),
            Err(MachineError::Protocol(_))
        ));
    }

    #[test]
    fn an_empty_payload_yields_no_chords() {
        assert_eq!(decode_chords(&[]).unwrap().len(), 0);
    }
}
