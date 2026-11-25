use super::*;

const SR: usize = SAMPLE_RATE as usize;

/// Deterministic "speech": a 220 Hz tone with a slow amplitude wobble so
/// frame energies are never identical.
fn speech(seconds: f64, amplitude: f32) -> Vec<f32> {
    let n = (seconds * SR as f64) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            let wobble = 0.8 + 0.2 * (t * 3.0).sin();
            amplitude * wobble * (2.0 * std::f32::consts::PI * 220.0 * t).sin()
        })
        .collect()
}

fn silence(seconds: f64) -> Vec<f32> {
    // Faint deterministic hiss, well under the speech floor.
    let n = (seconds * SR as f64) as usize;
    (0..n)
        .map(|i| 0.0005 * (((i * 7919) % 101) as f32 / 50.0 - 1.0))
        .collect()
}

fn run(chunks: &[Vec<f32>], chunk_len: usize) -> Vec<Window> {
    let mut windower = VadWindower::new(VadWindowConfig::default());
    let audio: Vec<f32> = chunks.iter().flatten().copied().collect();
    let mut windows = Vec::new();
    for piece in audio.chunks(chunk_len) {
        windows.extend(windower.push(piece));
    }
    windows.extend(windower.flush());
    windows
}

#[test]
fn single_utterance_becomes_one_window_with_padding() {
    let windows = run(&[silence(1.0), speech(2.0, 0.1), silence(2.0)], 1600);
    assert_eq!(windows.len(), 1);
    let w = &windows[0];
    // Pre-roll of ~200 ms before the speech at 1.0 s.
    assert!(
        (w.start_seconds() - 0.8).abs() < 0.05,
        "{}",
        w.start_seconds()
    );
    // 2 s of speech plus pre-roll and a short tail.
    assert!(w.duration_seconds() > 2.2 && w.duration_seconds() < 2.8);
}

#[test]
fn long_pause_splits_and_short_pause_does_not() {
    let two = run(
        &[
            speech(1.5, 0.1),
            silence(1.2),
            speech(1.5, 0.1),
            silence(1.5),
        ],
        1600,
    );
    assert_eq!(two.len(), 2);
    assert!(two[1].start_seconds() > two[0].start_seconds() + two[0].duration_seconds());

    let one = run(
        &[
            speech(1.5, 0.1),
            silence(0.4),
            speech(1.5, 0.1),
            silence(1.5),
        ],
        1600,
    );
    assert_eq!(one.len(), 1);
    assert!(one[0].duration_seconds() > 3.3);
}

#[test]
fn clicks_and_pure_silence_produce_nothing() {
    assert!(run(&[silence(5.0)], 1600).is_empty());
    let click = run(&[silence(1.0), speech(0.04, 0.3), silence(2.0)], 1600);
    assert!(click.is_empty());
}

#[test]
fn continuous_speech_is_force_cut_under_the_cap() {
    let windows = run(&[speech(70.0, 0.1), silence(2.0)], 3200);
    assert!(windows.len() >= 3, "{}", windows.len());
    for w in &windows {
        assert!(w.duration_seconds() <= 24.2, "{}", w.duration_seconds());
    }
    // Windows tile the audio without overlap or gaps larger than a frame.
    for pair in windows.windows(2) {
        let end = pair[0].start_sample + pair[0].samples.len() as u64;
        assert!(pair[1].start_sample >= end, "overlap");
        assert!(pair[1].start_sample - end <= SR as u64 / 2, "gap");
    }
}

#[test]
fn forced_cut_lands_on_the_quietest_frame_near_the_cap() {
    // 22 s of speech, a 100 ms dip, then 8 s more: the cut should be in the dip.
    let mut dip = speech(0.1, 0.1);
    dip.iter_mut().for_each(|s| *s *= 0.05);
    let windows = run(
        &[speech(22.0, 0.1), dip, speech(8.0, 0.1), silence(2.0)],
        3200,
    );
    assert!(windows.len() >= 2);
    let first_end = windows[0].start_seconds() + windows[0].duration_seconds();
    assert!((first_end - 22.1).abs() < 0.3, "{first_end}");
}

#[test]
fn result_does_not_depend_on_chunk_size() {
    let audio = [
        silence(0.5),
        speech(2.0, 0.1),
        silence(1.0),
        speech(1.0, 0.08),
        silence(1.0),
    ];
    let a = run(&audio, 160);
    let b = run(&audio, 12_345);
    assert_eq!(a, b);
    assert_eq!(a.len(), 2);
}

#[test]
fn flush_returns_speech_in_progress() {
    let mut w = VadWindower::new(VadWindowConfig::default());
    assert!(w.push(&speech(1.0, 0.1)).is_empty());
    let window = w.flush().expect("tail window");
    assert!(window.duration_seconds() > 0.9);
    assert!(w.flush().is_none());
}
