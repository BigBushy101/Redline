//! OS integration: notifications, keeping the PC awake, sleep and shutdown.

use serde::{Deserialize, Serialize};

/// Shows a desktop notification without blocking the UI. Does nothing on Android, where the
/// app posts its own.
pub fn notify(title: String, body: String) {
    #[cfg(target_os = "android")]
    let _ = (title, body);
    #[cfg(not(target_os = "android"))]
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new()
            .appname("Redline")
            .summary(&title)
            .body(&body)
            .show();
    });
}

/// Stops Windows from sleeping while `on` (the display may still turn off). A no-op elsewhere;
/// on Android the app holds its own wake lock.
pub fn keep_awake(on: bool) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Power::{
            ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
        };
        let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS };
        // SAFETY: plain Win32 call with valid flags.
        unsafe { SetThreadExecutionState(flags) };
    }
    #[cfg(not(windows))]
    let _ = on;
}

/// What to do once every queued job has finished. Deliberately not saved across sessions.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum AfterQueue {
    #[default]
    Nothing,
    OpenFolder,
    Sleep,
    ShutDown,
}

/// Seconds Windows waits before shutting down, so it can still be cancelled.
pub const SHUTDOWN_DELAY_SECS: u32 = 60;

pub fn sleep_now() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Power::SetSuspendState;
        // SAFETY: plain Win32 call; sleep (not hibernate), not forced, wake events allowed.
        unsafe { SetSuspendState(false as _, false as _, false as _) };
    }
}

pub fn schedule_shutdown() -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let status = crate::tools::command(std::path::Path::new("shutdown"))
            .args(["/s", "/t", &SHUTDOWN_DELAY_SECS.to_string(), "/c", "Redline finished its queue."])
            .status()?;
        if !status.success() {
            return Err(std::io::Error::other(format!("shutdown exited with {status}")));
        }
    }
    // Apps can't shut an Android phone down.
    #[cfg(target_os = "android")]
    return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "Android apps can't shut down the device"));
    #[cfg(not(target_os = "android"))]
    Ok(())
}

pub fn cancel_shutdown() {
    #[cfg(windows)]
    {
        let _ = crate::tools::command(std::path::Path::new("shutdown")).arg("/a").status();
    }
}
