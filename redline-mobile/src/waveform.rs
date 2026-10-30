//! Waveform peaks for Kotlin: ffmpeg analysis of the track's local copy or audio stream.

use redline_core::waveform as core_waveform;

use crate::music::Music;
use crate::records::TrackRecord;
use crate::RedlineError;

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct WaveformRecord {
    /// 0 to 1, one per bar.
    pub peaks: Vec<f32>,
    pub duration_secs: Option<f64>,
}

#[uniffi::export]
impl Music {
    /// Decodes the track once and returns its cached peaks. May resolve a link and download
    /// its audio the first time; call this off the UI thread.
    pub fn waveform_for_track(&self, track: TrackRecord) -> Result<WaveformRecord, RedlineError> {
        let (tools, net, source, key, duration) = {
            let st = self.inner.engine.state();
            let t = track.to_core();
            let (source, key) = core_waveform::source_and_key(&t, &st.history);
            (st.tools.clone(), st.settings.network.clone(), source, key, t.duration)
        };
        let wave = core_waveform::for_source(&tools, &net, &source, &key, duration)
            .map_err(RedlineError::failed)?;
        Ok(WaveformRecord { peaks: wave.peaks, duration_secs: wave.duration })
    }
}
