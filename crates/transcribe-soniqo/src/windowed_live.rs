//! Live transcription with a batch model on VAD windows (`ParakeetV3Live`).
//!
//! Each audio source (microphone = "me", system audio = "them") has its own
//! [`VadWindower`], so the two never mix and each result carries the channel
//! of the source it came from. Finished windows are transcribed immediately
//! and returned as final [`LivePartial`]s positioned on the source clock.

use crate::{
    LivePartial, Result, TranscriptSource,
    vad_window::{VadWindowConfig, VadWindower, Window},
};

/// Turns one window of 16 kHz mono samples into text.
pub(crate) trait SamplesTranscriber {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String>;
}

pub(crate) struct WindowedLive<T> {
    transcriber: T,
    microphone: VadWindower,
    system: VadWindower,
}

impl<T: SamplesTranscriber> WindowedLive<T> {
    pub(crate) fn new(transcriber: T) -> Self {
        Self::with_config(transcriber, VadWindowConfig::default())
    }

    pub(crate) fn with_config(transcriber: T, config: VadWindowConfig) -> Self {
        Self {
            transcriber,
            microphone: VadWindower::new(config.clone()),
            system: VadWindower::new(config),
        }
    }

    pub(crate) fn append(
        &mut self,
        source: TranscriptSource,
        samples: &[f32],
    ) -> Result<Vec<LivePartial>> {
        let windows = self.windower(source).push(samples);
        self.transcribe_windows(source, windows)
    }

    pub(crate) fn finalize(&mut self, source: TranscriptSource) -> Result<Vec<LivePartial>> {
        let windows: Vec<Window> = self.windower(source).flush().into_iter().collect();
        self.transcribe_windows(source, windows)
    }

    fn windower(&mut self, source: TranscriptSource) -> &mut VadWindower {
        match source {
            TranscriptSource::Microphone => &mut self.microphone,
            TranscriptSource::System => &mut self.system,
        }
    }

    fn transcribe_windows(
        &mut self,
        source: TranscriptSource,
        windows: Vec<Window>,
    ) -> Result<Vec<LivePartial>> {
        let mut partials = Vec::new();
        for window in windows {
            let text = self.transcriber.transcribe(&window.samples)?;
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            partials.push(LivePartial {
                source: source_label(source).to_string(),
                text: text.to_string(),
                is_final: true,
                start_seconds: Some(window.start_seconds()),
                duration_seconds: Some(window.duration_seconds()),
            });
        }
        Ok(partials)
    }
}

fn source_label(source: TranscriptSource) -> &'static str {
    match source {
        TranscriptSource::Microphone => "microphone",
        TranscriptSource::System => "system",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: usize = 16_000;

    /// Reports how many samples each window had.
    struct Echo {
        calls: Vec<usize>,
    }

    impl SamplesTranscriber for Echo {
        fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
            self.calls.push(samples.len());
            Ok(format!("window of {} samples", samples.len()))
        }
    }

    struct Blank;

    impl SamplesTranscriber for Blank {
        fn transcribe(&mut self, _samples: &[f32]) -> Result<String> {
            Ok("  ".to_string())
        }
    }

    fn tone(seconds: f64) -> Vec<f32> {
        (0..(seconds * SR as f64) as usize)
            .map(|i| 0.1 * (i as f32 * 0.09).sin())
            .collect()
    }

    fn quiet(seconds: f64) -> Vec<f32> {
        vec![0.0; (seconds * SR as f64) as usize]
    }

    #[test]
    fn sources_are_windowed_independently_and_keep_their_channel() {
        let mut live = WindowedLive::new(Echo { calls: vec![] });

        // "Them" speaks while "me" stays silent.
        let mut system = tone(1.5);
        system.extend(quiet(1.5));
        let partials = live.append(TranscriptSource::System, &system).unwrap();
        assert_eq!(partials.len(), 1);
        assert_eq!(partials[0].source(), TranscriptSource::System);
        assert!(partials[0].is_final);

        // Silence on the mic never produces text, even after speech on the other source.
        assert!(
            live.append(TranscriptSource::Microphone, &quiet(3.0))
                .unwrap()
                .is_empty()
        );
        assert!(
            live.finalize(TranscriptSource::Microphone)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn partials_carry_source_clock_timing() {
        let mut live = WindowedLive::new(Echo { calls: vec![] });
        let mut audio = quiet(2.0);
        audio.extend(tone(1.0));
        audio.extend(quiet(1.5));
        let partials = live.append(TranscriptSource::Microphone, &audio).unwrap();
        assert_eq!(partials.len(), 1);
        let start = partials[0].start_seconds.unwrap();
        let duration = partials[0].duration_seconds.unwrap();
        assert!((start - 1.8).abs() < 0.05, "{start}");
        assert!(duration > 1.1 && duration < 1.6, "{duration}");

        // The timing survives conversion to a stream response.
        let response =
            partials[0]
                .clone()
                .into_stream_response(crate::SoniqoModel::ParakeetV3Live, 99.0, 0.1);
        match response {
            owhisper_interface::stream::StreamResponse::TranscriptResponse {
                start: s,
                duration: d,
                channel_index,
                ..
            } => {
                assert!((s - start).abs() < 1e-9);
                assert!((d - duration).abs() < 1e-9);
                assert_eq!(channel_index, vec![0, 2]);
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn finalize_flushes_speech_that_has_not_ended() {
        let mut live = WindowedLive::new(Echo { calls: vec![] });
        assert!(
            live.append(TranscriptSource::System, &tone(2.0))
                .unwrap()
                .is_empty()
        );
        let partials = live.finalize(TranscriptSource::System).unwrap();
        assert_eq!(partials.len(), 1);
        assert_eq!(partials[0].source(), TranscriptSource::System);
    }

    #[test]
    fn empty_transcripts_are_dropped() {
        let mut live = WindowedLive::new(Blank);
        let mut audio = tone(1.0);
        audio.extend(quiet(1.5));
        assert!(
            live.append(TranscriptSource::Microphone, &audio)
                .unwrap()
                .is_empty()
        );
    }
}
