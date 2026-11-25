use super::*;

fn probs(pairs: &[(&str, f32)]) -> LangProbs {
    LangProbs::new(pairs.iter().map(|(c, p)| (*c, *p)))
}

fn whisper(lang: Option<&str>) -> Route {
    Route::Whisper {
        lang: lang.map(str::to_string),
    }
}

#[test]
fn preference_parses_and_defaults_to_auto() {
    assert_eq!("auto".parse(), Ok(RoutingPreference::Auto));
    assert_eq!(" Parakeet ".parse(), Ok(RoutingPreference::Parakeet));
    assert_eq!("WHISPER".parse(), Ok(RoutingPreference::Whisper));
    assert_eq!(
        RoutingPreference::parse_or_default(Some("nope")),
        RoutingPreference::Auto
    );
    assert_eq!(
        RoutingPreference::parse_or_default(None),
        RoutingPreference::Auto
    );
}

#[test]
fn probs_normalize_and_merge_chinese_variants() {
    let p = probs(&[
        ("zh-CN", 2.0),
        ("yue", 2.0),
        ("en", 4.0),
        ("de", f32::NAN),
        ("fr", -1.0),
    ]);
    assert!((p.zh_share() - 0.5).abs() < 1e-6);
    assert!((p.get("en") - 0.5).abs() < 1e-6);
    assert_eq!(p.get("de"), 0.0);
    assert!(probs(&[]).is_empty());
}

#[test]
fn classify_english_goes_to_parakeet() {
    let cfg = RouterConfig::default();
    assert_eq!(
        classify(&probs(&[("en", 0.9), ("de", 0.05), ("zh", 0.05)]), &cfg),
        Some(Route::Parakeet)
    );
    assert_eq!(
        classify(&probs(&[("fr", 0.8), ("en", 0.2)]), &cfg),
        Some(Route::Parakeet)
    );
}

#[test]
fn classify_mandarin_pins_whisper_to_zh() {
    let cfg = RouterConfig::default();
    assert_eq!(
        classify(&probs(&[("zh", 0.93), ("en", 0.05), ("ja", 0.02)]), &cfg),
        Some(whisper(Some("zh")))
    );
}

#[test]
fn classify_mixed_zh_and_english_is_auto_detect() {
    let cfg = RouterConfig::default();
    assert_eq!(
        classify(&probs(&[("zh", 0.2), ("en", 0.8)]), &cfg),
        Some(whisper(None))
    );
    assert_eq!(
        classify(&probs(&[("zh", 0.55), ("en", 0.45)]), &cfg),
        Some(whisper(None))
    );
    assert_eq!(
        classify(&probs(&[("zh", 0.75), ("en", 0.25)]), &cfg),
        Some(whisper(None))
    );
    // Just under the mixed threshold stays on Parakeet.
    assert_eq!(
        classify(&probs(&[("zh", 0.19), ("en", 0.81)]), &cfg),
        Some(Route::Parakeet)
    );
}

#[test]
fn classify_unsupported_language_uses_whisper() {
    let cfg = RouterConfig::default();
    assert_eq!(
        classify(&probs(&[("ja", 0.9), ("en", 0.1)]), &cfg),
        Some(whisper(Some("ja")))
    );
    // English leads but 40% Japanese: Parakeet would mangle it.
    assert_eq!(
        classify(&probs(&[("en", 0.6), ("ja", 0.4)]), &cfg),
        Some(whisper(None))
    );
    assert_eq!(classify(&LangProbs::default(), &cfg), None);
}

#[test]
fn auto_waits_for_thirty_seconds_of_speech() {
    let mut r = Router::new(RoutingPreference::Auto);
    let en = probs(&[("en", 0.7), ("de", 0.3)]);
    assert_eq!(r.observe(10_000, &en), Decision::Pending);
    assert_eq!(r.observe(10_000, &en), Decision::Pending);
    assert_eq!(r.current(), None);
    assert_eq!(r.observe(10_000, &en), Decision::Decided(Route::Parakeet));
    assert_eq!(r.current(), Some(&Route::Parakeet));
}

#[test]
fn very_high_confidence_decides_early() {
    let mut r = Router::new(RoutingPreference::Auto);
    let zh = probs(&[("zh", 0.98), ("en", 0.02)]);
    assert_eq!(r.observe(5_000, &zh), Decision::Pending);
    assert_eq!(
        r.observe(5_000, &zh),
        Decision::Decided(whisper(Some("zh")))
    );
}

#[test]
fn ambiguous_audio_does_not_decide_early() {
    let mut r = Router::new(RoutingPreference::Auto);
    let p = probs(&[("zh", 0.6), ("en", 0.4)]);
    assert_eq!(r.observe(12_000, &p), Decision::Pending);
    assert_eq!(r.observe(20_000, &p), Decision::Decided(whisper(None)));
}

#[test]
fn aggregation_is_weighted_by_speech_duration() {
    let mut r = Router::new(RoutingPreference::Auto);
    // A short Chinese burst must not outweigh long English speech.
    r.observe(2_000, &probs(&[("zh", 0.95), ("en", 0.05)]));
    let d = r.observe(28_000, &probs(&[("en", 0.9), ("de", 0.1)]));
    assert_eq!(d, Decision::Decided(Route::Parakeet));
}

#[test]
fn overrides_never_wait_and_never_switch() {
    let mut p = Router::new(RoutingPreference::Parakeet);
    assert_eq!(p.current(), Some(&Route::Parakeet));
    let zh = probs(&[("zh", 1.0)]);
    for _ in 0..10 {
        assert_eq!(p.observe(30_000, &zh), Decision::Unchanged);
    }
    assert_eq!(p.current(), Some(&Route::Parakeet));

    let w = Router::new(RoutingPreference::Whisper);
    assert_eq!(w.current(), Some(&whisper(None)));
}

#[test]
fn switching_needs_consecutive_confident_contradictions() {
    let mut r = Router::new(RoutingPreference::Auto);
    let en = probs(&[("en", 0.97), ("de", 0.03)]);
    let zh = probs(&[("zh", 0.95), ("en", 0.05)]);
    assert!(matches!(
        r.observe(30_000, &en),
        Decision::Decided(Route::Parakeet)
    ));

    assert_eq!(r.observe(10_000, &zh), Decision::Unchanged);
    assert_eq!(r.observe(10_000, &zh), Decision::Unchanged);
    // Agreement resets the streak.
    assert_eq!(r.observe(10_000, &en), Decision::Unchanged);
    assert_eq!(r.observe(10_000, &zh), Decision::Unchanged);
    assert_eq!(r.observe(10_000, &zh), Decision::Unchanged);
    assert_eq!(
        r.observe(10_000, &zh),
        Decision::Switched(whisper(Some("zh")))
    );
    assert_eq!(r.current(), Some(&whisper(Some("zh"))));
}

#[test]
fn short_or_unsure_windows_do_not_vote() {
    let mut r = Router::new(RoutingPreference::Auto);
    assert!(matches!(
        r.observe(30_000, &probs(&[("en", 1.0)])),
        Decision::Decided(_)
    ));
    let short = probs(&[("zh", 0.99), ("en", 0.01)]);
    let unsure = probs(&[("ja", 0.5), ("ko", 0.2), ("vi", 0.15), ("th", 0.15)]);
    for _ in 0..10 {
        assert_eq!(r.observe(2_000, &short), Decision::Unchanged);
        assert_eq!(r.observe(10_000, &unsure), Decision::Unchanged);
    }
    assert_eq!(r.current(), Some(&Route::Parakeet));
}

#[test]
fn leaving_mixed_route_is_stickier() {
    let mut r = Router::new(RoutingPreference::Auto);
    assert!(matches!(
        r.observe(30_000, &probs(&[("zh", 0.4), ("en", 0.6)])),
        Decision::Decided(Route::Whisper { lang: None })
    ));
    let en = probs(&[("en", 0.97), ("de", 0.03)]);
    for _ in 0..5 {
        assert_eq!(r.observe(10_000, &en), Decision::Unchanged);
    }
    assert_eq!(r.observe(10_000, &en), Decision::Switched(Route::Parakeet));
}

#[test]
fn finish_forces_a_decision() {
    let mut r = Router::new(RoutingPreference::Auto);
    r.observe(8_000, &probs(&[("zh", 0.85), ("en", 0.15)]));
    assert_eq!(r.current(), None);
    assert_eq!(r.finish(), whisper(Some("zh")));
    assert_eq!(r.current(), Some(&whisper(Some("zh"))));

    let mut empty = Router::new(RoutingPreference::Auto);
    assert_eq!(empty.observe(0, &LangProbs::default()), Decision::Pending);
    assert_eq!(empty.finish(), Route::Parakeet);
}

#[test]
fn parakeet_language_list_comes_from_language_crate() {
    assert!(is_parakeet_language("en"));
    assert!(is_parakeet_language("de-DE"));
    assert!(!is_parakeet_language("zh"));
    assert!(!is_parakeet_language("ja"));
}
