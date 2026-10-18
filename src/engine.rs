//! Starts and stops the capture, output and League threads as one unit.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crate::config::Config;
use crate::{capture, lights, lol};

pub struct Engine {
    stop: Arc<AtomicBool>,
    capture: JoinHandle<()>,
    usb: JoinHandle<()>,
}

impl Engine {
    pub fn start(config: Config) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        // Capacity 1: the capture side never blocks on USB, it retries next tick instead.
        let (tx, rx) = sync_channel(1);
        // Unbounded: events are rare and none may be lost.
        let (event_tx, event_rx) = channel();
        let usb = thread::Builder::new()
            .name("usb".into())
            .spawn(move || lights::run(rx, event_rx, config.outputs))
            .expect("spawn usb thread");
        // Not joined: these only sleep or wait on loopback HTTP and hold no device.
        if config.lol {
            let (stop, tx) = (Arc::clone(&stop), event_tx.clone());
            thread::Builder::new()
                .name("lol".into())
                .spawn(move || lol::run(tx, &stop))
                .expect("spawn lol thread");
        }
        if config.lol_demo {
            let (stop, tx) = (Arc::clone(&stop), event_tx.clone());
            thread::Builder::new()
                .name("lol-demo".into())
                .spawn(move || lol::demo(tx, &stop))
                .expect("spawn lol demo thread");
        }
        drop(event_tx);
        let capture = {
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("capture".into())
                .spawn(move || capture::run(config.capture, tx, &stop))
                .expect("spawn capture thread")
        };
        Engine { stop, capture, usb }
    }

    /// Flag that stops the engine when set; for signal handlers.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    /// Blocks until the engine has been stopped through `stop_flag`.
    pub fn wait(self) {
        // Dropping the sender when capture ends lets the USB thread exit.
        let _ = self.capture.join();
        let _ = self.usb.join();
        log!("stopped");
    }

    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wait();
    }
}
