//! Latest-value mailboxes for the fan-out from the lights thread to one thread
//! per device: publishing never blocks, and a slow device only skips frames
//! that are already stale instead of delaying the others.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

struct Slot<T> {
    value: Option<T>,
    closed: bool,
}

struct Shared<T> {
    slot: Mutex<Slot<T>>,
    ready: Condvar,
}

pub struct Publisher<T>(Arc<Shared<T>>);
pub struct Subscriber<T>(Arc<Shared<T>>);

/// The publisher was dropped.
#[derive(Debug, PartialEq)]
pub struct Closed;

pub fn mailbox<T>() -> (Publisher<T>, Subscriber<T>) {
    let shared = Arc::new(Shared { slot: Mutex::new(Slot { value: None, closed: false }), ready: Condvar::new() });
    (Publisher(Arc::clone(&shared)), Subscriber(shared))
}

impl<T> Publisher<T> {
    /// Replaces any frame the device has not picked up yet.
    pub fn publish(&self, value: T) {
        self.0.slot.lock().unwrap().value = Some(value);
        self.0.ready.notify_one();
    }
}

impl<T> Drop for Publisher<T> {
    fn drop(&mut self) {
        self.0.slot.lock().unwrap().closed = true;
        self.0.ready.notify_one();
    }
}

impl<T> Subscriber<T> {
    /// Newest frame, or `Ok(None)` if none arrived within `timeout`. Frames
    /// published before the publisher closed are still delivered.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<T>, Closed> {
        let slot = self.0.slot.lock().unwrap();
        let (mut slot, _) =
            self.0.ready.wait_timeout_while(slot, timeout, |s| s.value.is_none() && !s.closed).unwrap();
        match slot.value.take() {
            Some(v) => Ok(Some(v)),
            None if slot.closed => Err(Closed),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_newest_frame_then_closes() {
        let (tx, rx) = mailbox();
        tx.publish(1);
        tx.publish(2);
        assert_eq!(rx.recv_timeout(Duration::ZERO), Ok(Some(2)));
        assert_eq!(rx.recv_timeout(Duration::from_millis(1)), Ok(None));
        tx.publish(3);
        drop(tx);
        assert_eq!(rx.recv_timeout(Duration::ZERO), Ok(Some(3)));
        assert_eq!(rx.recv_timeout(Duration::ZERO), Err(Closed));
    }
}
