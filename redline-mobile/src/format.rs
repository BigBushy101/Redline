//! The desktop's text helpers, so the Android screens word sizes, times and trim problems the
//! same way. Only the pure ones: dates and clock times are formatted in Kotlin, with the
//! phone's own time zone and locale.

use redline_core::format;

use crate::records::{TrimRecord, check_trim};

/// "12.3 MB".
#[uniffi::export]
pub fn fmt_bytes(bytes: u64) -> String {
    format::fmt_bytes(bytes)
}

/// "3:07" or "1:02:03".
#[uniffi::export]
pub fn fmt_duration(secs: f64) -> String {
    format::fmt_duration(secs)
}

/// Like [`fmt_duration`], keeping tenths when the time isn't whole (the trim fields).
#[uniffi::export]
pub fn fmt_time(secs: f64) -> String {
    format::fmt_time(secs)
}

/// Parses `90`, `1:30`, `1:02:03` or `12.5` into seconds; `None` if it doesn't parse.
#[uniffi::export]
pub fn parse_duration(text: String) -> Option<f64> {
    format::parse_duration(&text)
}

/// Why `trim` can't be sent with a download, as the Download page warns before queueing;
/// `None` when it can. The same checks `Engine::queue_download` makes.
#[uniffi::export]
pub fn trim_problem(trim: TrimRecord, duration_secs: Option<f64>) -> Option<String> {
    check_trim(Some(trim), duration_secs).err()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_problems_match_the_engine() {
        let t = |start_secs, end_secs| TrimRecord { start_secs, end_secs, precise: false };
        assert_eq!(trim_problem(t(5.0, Some(20.0)), Some(60.0)), None);
        assert!(trim_problem(t(9.0, Some(3.0)), Some(60.0)).is_some());
        assert!(trim_problem(t(0.0, Some(60.0)), Some(60.0)).is_some(), "keeps everything");
        assert_eq!(parse_duration("1:30".into()), Some(90.0));
        assert_eq!(fmt_time(90.5), "1:30.5");
    }
}
