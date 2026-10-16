use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui;

/// A one-shot background computation whose result the UI polls each frame.
pub struct Task<T>(Arc<Mutex<Option<T>>>);

impl<T: Send + 'static> Task<T> {
    pub fn spawn(ctx: &egui::Context, f: impl FnOnce() -> T + Send + 'static) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let out = slot.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = f();
            *out.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
        Self(slot)
    }

    pub fn take(&self) -> Option<T> {
        self.0.lock().unwrap().take()
    }
}
