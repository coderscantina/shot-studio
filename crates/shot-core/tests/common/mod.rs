//! Helpers shared by the integration tests.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Runs `work` on a worker thread and returns its result, or panics naming `input` when it takes
/// longer than `budget`, so a parser that hangs fails the test instead of stalling CI. The stuck
/// thread is left behind and ends with the test process.
pub fn within<T: Send + 'static>(
    budget: Duration,
    input: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        tx.send(work()).ok();
    });
    match rx.recv_timeout(budget) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("still running after {budget:?} on {input}")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("panicked on {input}"),
    }
}
