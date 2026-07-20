//! The contract every protocol implements, and what the app hears back.

use pluvialis_core::Stroke;

#[derive(Debug, thiserror::Error)]
pub enum MachineError {
    /// No such device is attached right now.
    ///
    /// This is the single most important variant in the crate. Absent hardware
    /// is the normal idle state, not a failure: it happens once per scan,
    /// forever, while the writer is switched off. It must never be logged as a
    /// warning and must never end a scan loop.
    #[error("no device attached")]
    NotAttached,

    #[error("{0}")]
    Io(String),

    /// The device is attached but is not speaking the protocol we expect.
    #[error("protocol error: {0}")]
    Protocol(String),
}

impl MachineError {
    /// Whether this error means "nothing is plugged in", as opposed to
    /// something going wrong with a device that is.
    pub fn is_absence(&self) -> bool {
        matches!(self, MachineError::NotAttached)
    }
}

/// What the app shows in the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineStatus {
    /// Scanning, and will keep scanning. Not an error state.
    Searching,
    Connected {
        machine: String,
        port: String,
    },
    /// A connected machine went away. The scanner is already looking again.
    Disconnected {
        reason: String,
    },
}

/// Sent from the machine thread to the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineEvent {
    Status(MachineStatus),
    Stroke(Stroke),
}

/// One steno protocol.
///
/// Implementations are driven by the scanner, which owns the retry policy so
/// that no protocol has to reimplement it (and so no protocol can get it wrong
/// by giving up). An implementation only has to answer three questions: can I
/// open a device right now, what strokes have arrived, and how do I let go.
pub trait Machine: Send {
    /// Human readable, for the status bar.
    fn name(&self) -> &'static str;

    /// Try to open an attached device.
    ///
    /// Returns the port or device path on success, for display. Returning
    /// [`MachineError::NotAttached`] means "not now, ask me again", and the
    /// scanner will.
    fn connect(&mut self) -> Result<String, MachineError>;

    /// Collect whatever strokes have arrived.
    ///
    /// Blocks up to a short internal timeout and returns an empty vector if
    /// nothing came, so an idle machine costs one wakeup per timeout rather
    /// than a spin. Any error is treated by the scanner as the device having
    /// gone away.
    fn poll(&mut self) -> Result<Vec<Stroke>, MachineError>;

    /// Release the device. Must be safe to call when not connected.
    fn disconnect(&mut self);
}
