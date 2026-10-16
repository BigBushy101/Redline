use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui;
use redline_core::Wake;

/// A [`Wake`] that repaints the window.
pub fn waker(ctx: &egui::Context) -> Wake {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

/// A one-shot background computation whose result the UI polls each frame.
pub struct Task<T>(Arc<Mutex<Option<T>>>);

impl<T: Send + 'static> Task<T> {
    pub fn spawn(wake: &Wake, f: impl FnOnce() -> T + Send + 'static) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let out = slot.clone();
        let wake = wake.clone();
        thread::spawn(move || {
            let result = f();
            *out.lock().unwrap() = Some(result);
            wake();
        });
        Self(slot)
    }

    pub fn take(&self) -> Option<T> {
        self.0.lock().unwrap().take()
    }
}
