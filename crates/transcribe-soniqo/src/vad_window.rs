//! Energy-based voice-activity windowing for live batch-model transcription.
//!
//! Parakeet-TDT-v3 is an offline model (it sees up to ~30 s at a time). For
//! live use we cut the incoming audio into utterance-sized windows at pauses
//! and transcribe each window once it ends. One [`VadWindower`] per audio
//! source keeps the microphone ("me") and system audio ("them") separate.
//!
//! The detector is deliberately simple (frame RMS against an adaptive noise
//! floor): meetings are mostly clean speech or digital silence, and a missed
//! pause only makes a window longer, never wrong. Windows are cut:
//!
//! * after [`VadWindowConfig::endpoint_silence_ms`] of silence, or
//! * at the quietest frame near [`VadWindowConfig::max_window_ms`] when
//!   somebody talks without pausing.

use std::collections::VecDeque;

pub const SAMPLE_RATE: u64 = 16_000;

#[derive(Debug, Clone)]
pub struct VadWindowConfig {
    pub frame_ms: u64,
    /// Absolute RMS floor below which a frame is never speech.
    pub min_speech_rms: f32,
    /// A frame is speech when RMS exceeds `noise_floor * noise_ratio`.
    pub noise_ratio: f32,
    /// Silence that ends a window.
    pub endpoint_silence_ms: u64,
    /// Windows with less speech than this are dropped (clicks, bumps).
    pub min_window_speech_ms: u64,
    /// Hard cap; Parakeet-v3 handles at most ~29.5 s per call.
    pub max_window_ms: u64,
    /// How far back from the cap to look for the quietest frame to cut at.
    pub max_cut_search_ms: u64,
    /// Audio kept before the first speech frame so onsets are not clipped.
    pub preroll_ms: u64,
    /// Trailing silence kept after the last speech frame.
    pub tail_ms: u64,
}

impl Default for VadWindowConfig {
    fn default() -> Self {
        Self {
            frame_ms: 20,
            min_speech_rms: 0.006,
            noise_ratio: 3.0,
            endpoint_silence_ms: 700,
            min_window_speech_ms: 250,
            max_window_ms: 24_000,
            max_cut_search_ms: 4_000,
            preroll_ms: 200,
            tail_ms: 200,
        }
    }
}

/// One utterance-sized chunk of audio and where it sits on the source clock.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// Index of the first sample on the source's clock (16 kHz samples).
    pub start_sample: u64,
    pub samples: Vec<f32>,
}

impl Window {
    pub fn start_seconds(&self) -> f64 {
        self.start_sample as f64 / SAMPLE_RATE as f64
    }

    pub fn duration_seconds(&self) -> f64 {
        self.samples.len() as f64 / SAMPLE_RATE as f64
    }
}

#[derive(Debug, Clone)]
struct Frame {
    samples: Vec<f32>,
    rms: f32,
    speech: bool,
}

#[derive(Debug)]
pub struct VadWindower {
    config: VadWindowConfig,
    frame_len: usize,
    carry: Vec<f32>,
    /// Samples consumed into frames so far.
    position: u64,
    noise_floor: f32,
    preroll: VecDeque<Frame>,
    /// Frames of the window being built; empty while idle.
    current: Vec<Frame>,
    current_start: u64,
}

impl VadWindower {
    pub fn new(config: VadWindowConfig) -> Self {
        let frame_len = (SAMPLE_RATE * config.frame_ms / 1000) as usize;
        Self {
            config,
            frame_len,
            carry: Vec::new(),
            position: 0,
            noise_floor: 0.002,
            preroll: VecDeque::new(),
            current: Vec::new(),
            current_start: 0,
        }
    }

    /// Feeds audio and returns every window that completed.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Window> {
        self.carry.extend_from_slice(samples);
        let mut windows = Vec::new();
        let mut offset = 0;
        while self.carry.len() - offset >= self.frame_len {
            let frame_samples = self.carry[offset..offset + self.frame_len].to_vec();
            offset += self.frame_len;
            self.process_frame(frame_samples, &mut windows);
        }
        self.carry.drain(..offset);
        windows
    }

    /// Ends the stream: returns the window in progress, if it holds speech.
    pub fn flush(&mut self) -> Option<Window> {
        let window = self.finish_current();
        self.preroll.clear();
        self.carry.clear();
        window
    }

    fn frames_ms(&self, frames: usize) -> u64 {
        frames as u64 * self.config.frame_ms
    }

    fn process_frame(&mut self, samples: Vec<f32>, out: &mut Vec<Window>) {
        let rms = rms(&samples);
        let threshold = self
            .config
            .min_speech_rms
            .max(self.noise_floor * self.config.noise_ratio);
        let speech = rms > threshold;
        // Only non-speech frames move the floor, so long monologues cannot
        // teach the detector that speech is noise. In a room whose background
        // already exceeds the threshold nothing reads as silence: windows then
        // simply run to the length cap and are cut at their quietest frame.
        if !speech {
            self.update_noise_floor(rms);
        }
        let frame = Frame {
            samples,
            rms,
            speech,
        };
        let frame_start = self.position;
        self.position += self.frame_len as u64;

        if self.current.is_empty() {
            if !speech {
                self.preroll.push_back(frame);
                let max_preroll = (self.config.preroll_ms / self.config.frame_ms) as usize;
                while self.preroll.len() > max_preroll {
                    self.preroll.pop_front();
                }
                return;
            }
            let preroll_frames = self.preroll.len() as u64;
            self.current_start = frame_start - preroll_frames * self.frame_len as u64;
            self.current.extend(self.preroll.drain(..));
            self.current.push(frame);
            return;
        }

        self.current.push(frame);

        if self.trailing_silence_ms() >= self.config.endpoint_silence_ms {
            if let Some(window) = self.finish_current() {
                out.push(window);
            }
            return;
        }

        if self.frames_ms(self.current.len()) >= self.config.max_window_ms {
            self.force_cut(out);
        }
    }

    fn update_noise_floor(&mut self, rms: f32) {
        self.noise_floor = if rms < self.noise_floor {
            0.5 * self.noise_floor + 0.5 * rms
        } else {
            self.noise_floor + (rms - self.noise_floor) * 0.0005
        }
        .clamp(0.0005, 0.02);
    }

    fn trailing_silence_ms(&self) -> u64 {
        let frames = self
            .current
            .iter()
            .rev()
            .take_while(|frame| !frame.speech)
            .count();
        self.frames_ms(frames)
    }

    /// Splits at the quietest frame in the last `max_cut_search_ms`; the rest
    /// continues as the next window.
    fn force_cut(&mut self, out: &mut Vec<Window>) {
        let search_frames = (self.config.max_cut_search_ms / self.config.frame_ms) as usize;
        let from = self.current.len().saturating_sub(search_frames);
        let cut = self.current[from..]
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.rms.total_cmp(&b.1.rms))
            .map(|(i, _)| from + i + 1)
            .unwrap_or(self.current.len());

        let rest = self.current.split_off(cut);
        let head_start = self.current_start;
        let head = std::mem::take(&mut self.current);
        if let Some(window) = self.build_window(head_start, head) {
            out.push(window);
        }

        self.current_start = head_start + cut as u64 * self.frame_len as u64;
        self.current = rest;
        // A remainder without speech would never end; treat it as idle.
        if !self.current.iter().any(|f| f.speech) {
            self.preroll = self.current.drain(..).collect();
        }
    }

    fn finish_current(&mut self) -> Option<Window> {
        let frames = std::mem::take(&mut self.current);
        self.build_window(self.current_start, frames)
    }

    fn build_window(&self, start: u64, mut frames: Vec<Frame>) -> Option<Window> {
        let last_speech = frames.iter().rposition(|f| f.speech)?;
        let speech_frames = frames.iter().filter(|f| f.speech).count();
        if self.frames_ms(speech_frames) < self.config.min_window_speech_ms {
            return None;
        }
        let keep = last_speech + 1 + (self.config.tail_ms / self.config.frame_ms) as usize;
        frames.truncate(keep.min(frames.len()));
        let samples = frames.into_iter().flat_map(|f| f.samples).collect();
        Some(Window {
            start_sample: start,
            samples,
        })
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests;
