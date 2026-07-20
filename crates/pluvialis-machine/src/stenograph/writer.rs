//! The Stenograph read loop.
//!
//! One [`Machine::poll`] is one turn of the loop: make sure the realtime file
//! is open, ask for the next slice of it, and turn whatever comes back into
//! strokes. Everything routine (writer off, user not writing yet, file closed)
//! resolves to "reset and try again", never to an error the user sees.

use std::thread;
use std::time::Duration;

use pluvialis_core::Stroke;

use crate::keymap::Keymap;
use crate::machine::{Machine, MachineError};
use crate::stenograph::packet::{
    self, ERROR_FILE_NOT_AVAILABLE, ERROR_FINISHED_READING_CLOSED_FILE, ERROR_NO_REALTIME_FILE,
    ERROR_UNABLE_TO_PERFORM, PACKET_OPEN_FILE, PACKET_READ_FILE, Response,
};
use crate::stenograph::short_device_label;
use crate::stenograph::transport::Transport;

/// How long to wait after a poll that produced nothing.
///
/// Only the idle path sleeps: a poll that produced strokes returns at once so
/// a burst drains without waiting. At 300wpm strokes are about 200ms apart, so
/// this never gates real writing.
const IDLE_POLL: Duration = Duration::from_millis(100);

/// Where we are in reading the realtime file.
#[derive(Debug, Default)]
struct ReadState {
    /// False until a zero length response says we have caught up to live.
    realtime: bool,
    /// False until an open request has succeeded.
    file_open: bool,
    /// How far into the file we have read.
    offset: u32,
}

pub struct Stenograph {
    transport: Option<Transport>,
    keymap: Keymap,
    state: ReadState,
    sequence: u32,
}

impl Default for Stenograph {
    fn default() -> Self {
        Self::new()
    }
}

impl Stenograph {
    pub fn new() -> Self {
        Stenograph {
            transport: None,
            keymap: Keymap::stentura(),
            state: ReadState::default(),
            sequence: 0,
        }
    }

    fn next_sequence(&mut self) -> u32 {
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        sequence
    }

    /// Send a request and check the response belongs to it.
    fn exchange(
        &mut self,
        request: Vec<u8>,
        sequence: u32,
        expected_type: u16,
    ) -> Result<(Response, Vec<u8>), MachineError> {
        let transport = self
            .transport
            .as_mut()
            .ok_or_else(|| MachineError::Io("not connected".to_owned()))?;

        let (response, payload) = transport.send_receive(&request)?;

        if response.is_error() {
            // An error response carries its own type (0x06) rather than
            // echoing the request's, so the type check cannot apply and the
            // sequence is not worth rejecting on: the packet is
            // self-describing and the caller handles the code. Plover checks
            // both before looking at the code, so every error becomes an
            // unhandled protocol violation that kills its reader thread. Code
            // 8 simply means "not writing yet", which is the state the writer
            // is in most of the time, so that path is easy to hit.
            if response.sequence != sequence {
                log::debug!(
                    "error response sequence {} does not match request {sequence}",
                    response.sequence
                );
            }
            return Ok((response, payload));
        }

        if response.sequence != sequence {
            return Err(MachineError::Protocol(format!(
                "response sequence {} does not match request {sequence}",
                response.sequence
            )));
        }
        if response.packet_type != expected_type {
            return Err(MachineError::Protocol(format!(
                "response type {:#x} does not match request {expected_type:#x}",
                response.packet_type
            )));
        }

        Ok((response, payload))
    }

    /// Turn a payload into strokes, dropping chords that carry no steno.
    fn strokes_from(&self, payload: &[u8]) -> Result<Vec<Stroke>, MachineError> {
        let mut strokes = Vec::new();

        for keys in packet::decode_chords(payload)? {
            match self.keymap.stroke(&keys) {
                // A chord of only unbound keys, such as `^` on its own.
                Ok(None) => {}
                Ok(Some(stroke)) => strokes.push(stroke),
                // Skip rather than drop the connection: losing one odd chord
                // beats interrupting someone mid sentence.
                Err(e) => log::warn!("undecodable chord {keys:?}: {e}"),
            }
        }

        Ok(strokes)
    }
}

/// Whether an error code means "reset and carry on" rather than a real fault.
///
/// All four are routine. 8 (the user has not started writing) and 9 (the file
/// was closed) happen constantly. Treating either as a failure produces
/// software that looks broken whenever it is merely idle.
fn is_routine(code: u32) -> bool {
    matches!(
        code,
        ERROR_UNABLE_TO_PERFORM
            | ERROR_FILE_NOT_AVAILABLE
            | ERROR_NO_REALTIME_FILE
            | ERROR_FINISHED_READING_CLOSED_FILE
    )
}

impl Machine for Stenograph {
    fn name(&self) -> &'static str {
        "Stenograph USB"
    }

    fn connect(&mut self) -> Result<String, MachineError> {
        let transport = Transport::open()?;
        let label = short_device_label(transport.path());

        self.transport = Some(transport);
        self.state = ReadState::default();
        self.sequence = 0;

        Ok(label)
    }

    fn poll(&mut self) -> Result<Vec<Stroke>, MachineError> {
        if !self.state.file_open {
            let sequence = self.next_sequence();
            let request = packet::open_file_request(sequence);
            let (response, _) = self.exchange(request, sequence, PACKET_OPEN_FILE)?;

            if response.is_error() {
                let code = response.error_code();
                if is_routine(code) {
                    log::trace!("realtime file not open yet (code {code})");
                    self.state = ReadState::default();
                    thread::sleep(IDLE_POLL);
                    return Ok(Vec::new());
                }
                return Err(MachineError::Protocol(format!(
                    "unexpected error {code} opening the realtime file"
                )));
            }
            self.state.file_open = true;
        }

        let sequence = self.next_sequence();
        let request = packet::read_file_request(sequence, self.state.offset);
        let (response, payload) = self.exchange(request, sequence, PACKET_READ_FILE)?;

        if response.is_error() {
            let code = response.error_code();
            if is_routine(code) {
                log::trace!("read reset (code {code})");
                self.state = ReadState::default();
                thread::sleep(IDLE_POLL);
                return Ok(Vec::new());
            }
            return Err(MachineError::Protocol(format!(
                "unexpected error {code} reading the realtime file"
            )));
        }

        if payload.is_empty() {
            // Caught up to live. Everything from here on is the user writing
            // now, so this is the moment strokes start counting.
            if !self.state.realtime {
                log::info!("Stenograph writer is live");
                self.state.realtime = true;
            }
            thread::sleep(IDLE_POLL);
            return Ok(Vec::new());
        }

        self.state.offset += response.data_length;

        if !self.state.realtime {
            // The backlog already sitting in the file. Read past it and throw
            // it away, or connecting would dump the previous session's text
            // into the document. This looks like a bug and is deliberate.
            log::debug!("discarding {} bytes of backlog", payload.len());
            return Ok(Vec::new());
        }

        self.strokes_from(&payload)
    }

    fn disconnect(&mut self) {
        // Dropping the transport closes the handle, in that order.
        self.transport = None;
        self.state = ReadState::default();
    }
}
