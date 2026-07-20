//! The Auto scanner: the reason this project exists.
//!
//! Plover's Stenograph plugin gives up permanently if the writer is not present
//! at the moment capture starts, which is what forces the restart-and-reselect
//! ritual every session. The fix is not a better error message or a reconnect
//! button. It is to treat absent hardware as an ordinary state that the program
//! sits in indefinitely without complaint.
//!
//! So this loop has no give-up branch, by construction. It tries every known
//! protocol in priority order, sleeps, and tries again, forever. Turn the
//! machine on at any point and it connects. Unplug it mid-sentence and it goes
//! back to looking. Neither needs a click.
//!
//! **Do not add an attempt limit, a "machine not found" terminal state, or a
//! dialog.** Any of those reintroduces exactly the bug this replaces.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crossbeam_channel::Sender;

use crate::machine::{Machine, MachineEvent, MachineStatus};

/// How long to wait between scan rounds when nothing is attached.
///
/// One second is frequent enough to feel instant when the writer comes on, and
/// cheap enough that idling all day is free. The M4b soak test checks that an
/// idle scan holds CPU near zero and handle count flat.
const SCAN_INTERVAL: Duration = Duration::from_secs(1);

/// Handle to a running scanner. Dropping it stops the thread.
pub struct Scanner {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Scanner {
    /// Start scanning on a background thread.
    ///
    /// `machines` is the priority order: the first one that connects wins. The
    /// receiver gets status changes and strokes.
    pub fn spawn(machines: Vec<Box<dyn Machine>>, events: Sender<MachineEvent>) -> Scanner {
        let running = Arc::new(AtomicBool::new(true));
        let flag = running.clone();

        let handle = thread::Builder::new()
            .name("pluvialis-machine".to_owned())
            .spawn(move || scan_loop(machines, &events, &flag))
            .expect("spawning the machine thread");

        Scanner {
            running,
            handle: Some(handle),
        }
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Sleep in short slices so stopping is prompt without polling hard.
fn interruptible_sleep(total: Duration, running: &AtomicBool) {
    const SLICE: Duration = Duration::from_millis(50);
    let mut left = total;
    while left > Duration::ZERO && running.load(Ordering::Relaxed) {
        let slice = SLICE.min(left);
        thread::sleep(slice);
        left -= slice;
    }
}

fn scan_loop(
    mut machines: Vec<Box<dyn Machine>>,
    events: &Sender<MachineEvent>,
    running: &AtomicBool,
) {
    let mut announced_searching = false;

    while running.load(Ordering::Relaxed) {
        let mut connected = false;

        for machine in &mut machines {
            if !running.load(Ordering::Relaxed) {
                break;
            }

            match machine.connect() {
                Ok(port) => {
                    let name = machine.name();
                    log::info!("connected to {name} on {port}");
                    if events
                        .send(MachineEvent::Status(MachineStatus::Connected {
                            machine: name.to_owned(),
                            port,
                        }))
                        .is_err()
                    {
                        return;
                    }

                    announced_searching = false;
                    connected = true;
                    let reason = read_until_lost(machine.as_mut(), events, running);
                    machine.disconnect();

                    // A closed channel means the app is gone.
                    if reason.is_none() {
                        return;
                    }
                    let reason = reason.expect("checked");

                    log::info!("{name} disconnected: {reason}");
                    if events
                        .send(MachineEvent::Status(MachineStatus::Disconnected { reason }))
                        .is_err()
                    {
                        return;
                    }
                    break;
                }
                Err(e) if e.is_absence() => {
                    // Nothing plugged in. Routine, so trace level only: at one
                    // scan per second this would otherwise bury the log and
                    // make an idle program look broken.
                    log::trace!("{} not attached", machine.name());
                }
                Err(e) => log::debug!("{} could not connect: {e}", machine.name()),
            }
        }

        if !connected {
            // Announce searching once per dry spell, not once per scan.
            if !announced_searching {
                announced_searching = true;
                if events
                    .send(MachineEvent::Status(MachineStatus::Searching))
                    .is_err()
                {
                    return;
                }
            }
            interruptible_sleep(SCAN_INTERVAL, running);
        }
    }
}

/// Pump strokes until the device goes away or we are asked to stop.
///
/// Returns why it ended, or `None` if the app closed the channel.
fn read_until_lost(
    machine: &mut dyn Machine,
    events: &Sender<MachineEvent>,
    running: &AtomicBool,
) -> Option<String> {
    while running.load(Ordering::Relaxed) {
        match machine.poll() {
            Ok(strokes) => {
                for stroke in strokes {
                    events.send(MachineEvent::Stroke(stroke)).ok()?;
                }
            }
            Err(e) => return Some(e.to_string()),
        }
    }
    Some("stopped".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::MachineError;
    use pluvialis_core::Stroke;
    use std::sync::Mutex;

    /// A machine that is absent for the first `absent_for` connect attempts,
    /// then yields `strokes` once, then reports the device gone.
    struct FakeMachine {
        absent_for: usize,
        attempts: Arc<Mutex<usize>>,
        strokes: Vec<Stroke>,
        delivered: bool,
        connected: bool,
        disconnects: Arc<Mutex<usize>>,
    }

    impl Machine for FakeMachine {
        fn name(&self) -> &'static str {
            "Fake"
        }

        fn connect(&mut self) -> Result<String, MachineError> {
            let mut attempts = self.attempts.lock().unwrap();
            *attempts += 1;
            if *attempts <= self.absent_for {
                return Err(MachineError::NotAttached);
            }
            self.connected = true;
            Ok("FAKE1".to_owned())
        }

        fn poll(&mut self) -> Result<Vec<Stroke>, MachineError> {
            if !self.delivered {
                self.delivered = true;
                return Ok(self.strokes.clone());
            }
            Err(MachineError::Io("unplugged".to_owned()))
        }

        fn disconnect(&mut self) {
            self.connected = false;
            *self.disconnects.lock().unwrap() += 1;
        }
    }

    fn fake(
        absent_for: usize,
        outline: &str,
    ) -> (FakeMachine, Arc<Mutex<usize>>, Arc<Mutex<usize>>) {
        let attempts = Arc::new(Mutex::new(0));
        let disconnects = Arc::new(Mutex::new(0));
        let machine = FakeMachine {
            absent_for,
            attempts: attempts.clone(),
            strokes: Stroke::parse_outline(outline).unwrap(),
            delivered: false,
            connected: false,
            disconnects: disconnects.clone(),
        };
        (machine, attempts, disconnects)
    }

    /// Collect events until we see `wanted`, or time out.
    fn wait_for(
        rx: &crossbeam_channel::Receiver<MachineEvent>,
        wanted: impl Fn(&MachineEvent) -> bool,
    ) -> Option<MachineEvent> {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(event) if wanted(&event) => return Some(event),
                Ok(_) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(_) => return None,
            }
        }
        None
    }

    #[test]
    fn strokes_reach_the_app() {
        let (machine, _, _) = fake(0, "KAT");
        let (tx, rx) = crossbeam_channel::unbounded();
        let _scanner = Scanner::spawn(vec![Box::new(machine)], tx);

        let event = wait_for(&rx, |e| matches!(e, MachineEvent::Stroke(_)));
        match event {
            Some(MachineEvent::Stroke(stroke)) => {
                assert_eq!(Stroke::render_outline(&[stroke]), "KAT");
            }
            other => panic!("expected a stroke, got {other:?}"),
        }
    }

    #[test]
    fn connecting_is_announced() {
        let (machine, _, _) = fake(0, "KAT");
        let (tx, rx) = crossbeam_channel::unbounded();
        let _scanner = Scanner::spawn(vec![Box::new(machine)], tx);

        let event = wait_for(&rx, |e| {
            matches!(e, MachineEvent::Status(MachineStatus::Connected { .. }))
        });
        assert!(event.is_some(), "never announced a connection");
    }

    /// The whole point: absent hardware must not end the loop.
    #[test]
    fn an_absent_machine_is_retried_rather_than_given_up_on() {
        let (machine, attempts, _) = fake(3, "KAT");
        let (tx, rx) = crossbeam_channel::unbounded();
        let _scanner = Scanner::spawn(vec![Box::new(machine)], tx);

        // It must keep trying past the absent phase and eventually connect.
        let event = wait_for(&rx, |e| matches!(e, MachineEvent::Stroke(_)));
        assert!(event.is_some(), "never recovered from an absent machine");
        assert!(
            *attempts.lock().unwrap() > 3,
            "gave up before the machine appeared"
        );
    }

    #[test]
    fn searching_is_reported_while_nothing_is_attached() {
        let (machine, _, _) = fake(usize::MAX, "KAT");
        let (tx, rx) = crossbeam_channel::unbounded();
        let _scanner = Scanner::spawn(vec![Box::new(machine)], tx);

        let event = wait_for(&rx, |e| {
            matches!(e, MachineEvent::Status(MachineStatus::Searching))
        });
        assert!(event.is_some(), "never reported searching");
    }

    /// Losing the device must return to scanning, not stop.
    #[test]
    fn a_lost_device_leads_back_to_searching() {
        let (machine, _, disconnects) = fake(0, "KAT");
        let (tx, rx) = crossbeam_channel::unbounded();
        let _scanner = Scanner::spawn(vec![Box::new(machine)], tx);

        let event = wait_for(&rx, |e| {
            matches!(e, MachineEvent::Status(MachineStatus::Disconnected { .. }))
        });
        assert!(event.is_some(), "never reported the device going away");
        assert!(
            *disconnects.lock().unwrap() >= 1,
            "did not release the device"
        );
    }

    #[test]
    fn stopping_ends_the_thread_promptly() {
        let (machine, _, _) = fake(usize::MAX, "KAT");
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut scanner = Scanner::spawn(vec![Box::new(machine)], tx);

        let started = std::time::Instant::now();
        scanner.stop();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "stop took {:?}, the sleep is not interruptible",
            started.elapsed()
        );
    }
}
