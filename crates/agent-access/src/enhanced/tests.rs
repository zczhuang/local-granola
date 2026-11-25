use super::*;
use serde_json::json;

async fn fixture() -> anlg_db_core::Db {
    let db = anlg_db_core::Db::connect_memory_plain().await.unwrap();
    anlg_db_app::prepare_schema(&db).await.unwrap();
    sqlx::query(
        "INSERT INTO sessions (id, title, started_at, folder_path) VALUES
         ('m1', 'Launch planning', '2026-07-13', 'Clients/ACME'),
         ('m2', 'Weekly sync', '2026-07-12', ''),
         ('m3', 'Budget review', '2026-07-11', '')",
    )
    .execute(db.pool())
    .await
    .unwrap();

    let words = json!([
        {"id": "w1", "text": " We", "start_ms": 0, "end_ms": 300, "channel": 0},
        {"id": "w2", "text": " ship", "start_ms": 300, "end_ms": 600, "channel": 0},
        {"id": "w3", "text": " on", "start_ms": 600, "end_ms": 800, "channel": 0},
        {"id": "w4", "text": " Friday.", "start_ms": 800, "end_ms": 1200, "channel": 0},
        {"id": "w5", "text": " Dana", "start_ms": 65_000, "end_ms": 65_400, "channel": 1},
        {"id": "w6", "text": " owns", "start_ms": 65_400, "end_ms": 65_800, "channel": 1},
        {"id": "w7", "text": " the", "start_ms": 65_800, "end_ms": 66_000, "channel": 1},
        {"id": "w8", "text": " pricing", "start_ms": 66_000, "end_ms": 66_500, "channel": 1},
        {"id": "w9", "text": " page.", "start_ms": 66_500, "end_ms": 67_000, "channel": 1}
    ]);
    sqlx::query("INSERT INTO transcripts (id, session_id, words_json) VALUES ('t1', 'm1', ?)")
        .bind(words.to_string())
        .execute(db.pool())
        .await
        .unwrap();

    let note = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "Launch date"}]},
        {"type": "bulletList", "content": [
            {"type": "listItem", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "Pricing page"}]}
            ]}
        ]}
    ]});
    sqlx::query(
        "INSERT INTO session_documents (id, session_id, kind, body_format, body)
         VALUES ('m1', 'm1', 'note', 'prosemirror_json', ?)",
    )
    .bind(note.to_string())
    .execute(db.pool())
    .await
    .unwrap();

    let ai = |id: &str, text: &str, start: &str, end: &str, a: i64, b: i64| {
        json!({"type": "listItem", "attrs": {"aiId": id, "src": [
            {"transcript_id": "t1", "start_word_id": start, "end_word_id": end, "start_ms": a, "end_ms": b}
        ]}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": text}]}]})
    };
    let enhanced = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "Launch date"}]},
        {"type": "bulletList", "content": [ai("a1", "Ship on Friday", "w1", "w4", 0, 1200)]},
        {"type": "bulletList", "content": [
            {"type": "listItem", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "Pricing page"}]},
                {"type": "bulletList", "content": [ai("a2", "Dana owns it", "w5", "w9", 65_000, 67_000)]}
            ]}
        ]},
        {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Decisions"}]},
        {"type": "bulletList", "content": [ai("a3", "Friday launch", "w1", "w4", 0, 1200)]},
        {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Action items"}]},
        {"type": "taskList", "content": [
            {"type": "taskItem", "attrs": {"aiId": "a4", "checked": false, "src": [
                {"transcript_id": "t1", "start_word_id": "w5", "end_word_id": "w9", "start_ms": 65_000, "end_ms": 67_000}
            ]}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Finish pricing page"}]}]}
        ]},
        {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Open questions"}]},
        {"type": "bulletList", "content": [{"type": "listItem", "attrs": {"aiId": "a5", "src": [
            {"t": "t1", "s": "gone-1", "e": "gone-2", "a": 0, "b": 0}
        ]}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Who tests it?"}]}]}]}
    ]});
    sqlx::query(
        "INSERT INTO session_documents (id, session_id, kind, body_format, body, generation_metadata_json, updated_at)
         VALUES ('enh1', 'm1', 'summary', 'prosemirror_json', ?, ?, '2026-07-14T00:00:00Z')",
    )
    .bind(enhanced.to_string())
    .bind(json!({"model": "gemma", "action_items": [{"text": "Finish pricing page", "owner": "Dana", "due": "Friday"}]}).to_string())
    .execute(db.pool())
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO session_documents (id, session_id, kind, body_format, body)
         VALUES ('m2', 'm2', 'note', 'markdown', 'Discuss the offsite budget and 100% attendance')",
    )
    .execute(db.pool())
    .await
    .unwrap();
    db
}

#[tokio::test]
async fn enhanced_notes_resolve_sources_decisions_and_actions() {
    let db = fixture().await;
    let notes = get_enhanced_notes(
        db.pool(),
        GetEnhancedNotesInput {
            meeting_id: "m1".into(),
            document_id: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(notes.document_id.as_deref(), Some("enh1"));
    assert_eq!(notes.origin, "saved");
    assert_eq!(notes.generation["model"], "gemma");
    let texts: Vec<_> = notes.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["Launch date", "Pricing page"]);
    assert_eq!(notes.lines[1].kind, "list_item");

    let first = &notes.lines[0].ai_bullets[0];
    assert_eq!(first.ai_id, "a1");
    assert_eq!(first.text, "Ship on Friday");
    let source = &first.sources[0];
    assert!(source.resolved);
    assert_eq!(source.text, "We ship on Friday.");
    assert_eq!(
        (source.start.as_str(), source.end.as_str()),
        ("00:00", "00:01")
    );
    assert_eq!(source.speakers.len(), 1);

    let nested = &notes.lines[1].ai_bullets[0];
    assert_eq!(nested.text, "Dana owns it");
    assert_eq!(nested.sources[0].start, "01:05");
    assert_eq!(nested.sources[0].text, "Dana owns the pricing page.");
    assert_ne!(nested.sources[0].speakers, source.speakers);

    assert_eq!(notes.decisions[0].text, "Friday launch");
    let action = &notes.action_items[0];
    assert_eq!(action.text, "Finish pricing page");
    assert_eq!(action.owner.as_deref(), Some("Dana"));
    assert_eq!(action.due.as_deref(), Some("Friday"));
    assert!(!action.done);
    assert_eq!(notes.open_questions[0].text, "Who tests it?");
    assert!(!notes.open_questions[0].sources[0].resolved);
    assert_eq!(notes.unresolved_sources, 1);

    assert!(notes.markdown.contains("- Ship on Friday [^1]"));
    assert!(notes.markdown.contains("  - Dana owns it [^2]"));
    assert!(notes.markdown.contains("[^1]: [00:00–00:01]"));
    assert!(notes.markdown.contains("\"We ship on Friday.\""));
    assert!(
        notes
            .markdown
            .contains("- [ ] Finish pricing page (owner: Dana, due: Friday) [^2]")
    );

    let missing = get_enhanced_notes(
        db.pool(),
        GetEnhancedNotesInput {
            meeting_id: "m2".into(),
            document_id: None,
        },
    )
    .await;
    assert!(matches!(missing, Err(Error::NotFound(_))));
}

#[tokio::test]
async fn search_covers_title_notes_enhanced_and_transcript_with_pagination() {
    let db = fixture().await;
    let page = |query: &str, limit: Option<u32>, offset: Option<u32>| {
        search_meetings(
            db.pool(),
            SearchMeetingsInput {
                query: query.into(),
                folder_path: None,
                limit,
                offset,
            },
        )
    };

    let by_title = page("launch", None, None).await.unwrap();
    assert_eq!(by_title.hits[0].meeting_id, "m1");
    assert_eq!(by_title.hits[0].date, "2026-07-13");
    assert_eq!(
        by_title.hits[0].folder_path.as_deref(),
        Some("Clients/ACME")
    );
    let sources: Vec<_> = by_title.hits[0]
        .snippets
        .iter()
        .map(|s| s.source.as_str())
        .collect();
    assert!(sources.contains(&"title"));
    assert!(sources.contains(&"enhanced_notes"));

    let transcript = page("pricing page", None, None).await.unwrap();
    let snippet = transcript.hits[0]
        .snippets
        .iter()
        .find(|s| s.source == "transcript")
        .expect("transcript snippet");
    assert_eq!(snippet.time.as_deref(), Some("01:06"));
    assert_eq!(snippet.transcript_id.as_deref(), Some("t1"));

    // Words that only occur as JSON keys in stored rows are not matches.
    assert!(page("start_ms", None, None).await.unwrap().hits.is_empty());
    // LIKE wildcards are literal.
    let literal = page("100%", None, None).await.unwrap();
    assert_eq!(literal.hits.len(), 1);
    assert_eq!(literal.hits[0].meeting_id, "m2");
    assert_eq!(literal.hits[0].snippets[0].source, "typed_notes");
    assert!(page("%", None, None).await.unwrap().hits.len() == 1);

    // Pagination: "e" appears in several titles/notes.
    let first = page("e", Some(1), None).await.unwrap();
    assert_eq!(first.hits.len(), 1);
    assert_eq!(first.pagination.next_offset, Some(1));
    let second = page("e", Some(1), Some(1)).await.unwrap();
    assert_ne!(first.hits[0].meeting_id, second.hits[0].meeting_id);

    let scoped = search_meetings(
        db.pool(),
        SearchMeetingsInput {
            query: "e".into(),
            folder_path: Some("Clients".into()),
            limit: None,
            offset: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(scoped.hits.len(), 1);
    assert_eq!(scoped.hits[0].meeting_id, "m1");

    assert!(matches!(
        page("  ", None, None).await,
        Err(Error::Invalid(_))
    ));
}

#[tokio::test]
async fn transcript_segment_by_source_ref_and_time_range() {
    let db = fixture().await;
    let by_ref = get_transcript_segment(
        db.pool(),
        GetTranscriptSegmentInput {
            meeting_id: "m1".into(),
            transcript_id: Some("t1".into()),
            start_word_id: Some("w5".into()),
            end_word_id: Some("w9".into()),
            start_ms: None,
            end_ms: None,
            max_words: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(by_ref.segments.len(), 1);
    assert_eq!(by_ref.segments[0].text, "Dana owns the pricing page.");
    assert_eq!(by_ref.segments[0].start, "01:05");
    assert!(by_ref.text.starts_with("[01:05] "));
    assert_eq!(by_ref.word_count, 5);
    assert!(!by_ref.truncated);

    let by_time = get_transcript_segment(
        db.pool(),
        GetTranscriptSegmentInput {
            meeting_id: "m1".into(),
            transcript_id: None,
            start_word_id: None,
            end_word_id: None,
            start_ms: Some(0),
            end_ms: Some(66_000),
            max_words: Some(3),
        },
    )
    .await
    .unwrap();
    assert_eq!(by_time.word_count, 3);
    assert!(by_time.truncated);
    assert_eq!(by_time.segments[0].text, "We ship on");

    let both = get_transcript_segment(
        db.pool(),
        GetTranscriptSegmentInput {
            meeting_id: "m1".into(),
            transcript_id: None,
            start_word_id: None,
            end_word_id: None,
            start_ms: Some(0),
            end_ms: Some(66_000),
            max_words: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(both.segments.len(), 2);
    assert_ne!(both.segments[0].speaker, both.segments[1].speaker);

    for bad in [
        (None, None, None, None),
        (Some("w1".to_string()), None, None, None),
        (None, None, Some(10), Some(5)),
    ] {
        let result = get_transcript_segment(
            db.pool(),
            GetTranscriptSegmentInput {
                meeting_id: "m1".into(),
                transcript_id: None,
                start_word_id: bad.0,
                end_word_id: bad.1,
                start_ms: bad.2,
                end_ms: bad.3,
                max_words: None,
            },
        )
        .await;
        assert!(matches!(result, Err(Error::Invalid(_))));
    }
    let missing = get_transcript_segment(
        db.pool(),
        GetTranscriptSegmentInput {
            meeting_id: "nope".into(),
            transcript_id: None,
            start_word_id: None,
            end_word_id: None,
            start_ms: Some(0),
            end_ms: Some(1),
            max_words: None,
        },
    )
    .await;
    assert!(matches!(missing, Err(Error::NotFound(_))));
}

#[tokio::test]
async fn typed_lines_exclude_ai_and_preview_assembles_unsaved_notes() {
    let db = fixture().await;
    let enhanced: String =
        sqlx::query_scalar("SELECT body FROM session_documents WHERE id = 'enh1'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        typed_lines(&enhanced, "prosemirror_json"),
        ["Launch date", "Pricing page"]
    );
    assert_eq!(typed_lines("- one\n\n* two", "markdown"), ["one", "two"]);

    let inputs = load_enhance_inputs(db.pool(), "m1").await.unwrap();
    assert_eq!(inputs.typed_lines, ["Launch date", "Pricing page"]);
    let turns = inputs.index.turns();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].sid, "s1");
    assert_eq!(turns[1].start_word_id, "w5");

    assert_eq!(
        inputs.raw_doc["content"][0]["content"][0]["text"],
        "Launch date"
    );
    // The engine's document is parsed the way a saved one is: anchors stay, AI lines carry sources.
    let doc = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "Launch date"}]},
        {"type": "bulletList", "content": [
            {"type": "listItem", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "Pricing page"}]},
                {"type": "bulletList", "content": [
                    {"type": "listItem", "attrs": {"aiId": "preview-1", "src": [
                        {"t": "t1", "s": "w5", "e": "w9", "a": 65_000, "b": 67_000}
                    ]}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Dana owns it"}]}]}
                ]}
            ]}
        ]},
        {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Action items"}]},
        {"type": "taskList", "content": [
            {"type": "taskItem", "attrs": {"checked": false, "aiId": "preview-2", "src": [
                {"t": "t1", "s": "w5", "e": "w9", "a": 65_000, "b": 67_000}
            ]}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Finish pricing (owner: Dana; due: Friday)"}]}]}
        ]}
    ]});
    let notes = assemble_preview_from_doc(&inputs, &doc, json!({"model": "stub"}));
    assert_eq!(notes.action_items[0].owner.as_deref(), Some("Dana"));
    assert_eq!(notes.action_items[0].due.as_deref(), Some("Friday"));
    assert_eq!(notes.origin, "preview");
    assert!(notes.document_id.is_none());
    assert_eq!(
        notes.lines[1].ai_bullets[0].sources[0].text,
        "Dana owns the pricing page."
    );
    assert!(notes.markdown.contains("Dana owns it [^1]"));
}

#[test]
fn owner_and_due_are_read_from_the_engine_text_suffix() {
    assert_eq!(
        owner_due_suffix("Send deck (owner: Sam; due: Mon)"),
        (Some("Sam".into()), Some("Mon".into()))
    );
    assert_eq!(owner_due_suffix("x (due: Mon)"), (None, Some("Mon".into())));
    assert_eq!(
        owner_due_suffix("x (owner: Sam)"),
        (Some("Sam".into()), None)
    );
    assert_eq!(owner_due_suffix("plain"), (None, None));
    assert_eq!(owner_due_suffix("x (see appendix)"), (None, None));
}

#[test]
fn raw_doc_falls_back_to_the_typed_lines() {
    assert_eq!(
        doc_from_lines(&["a".into(), "b".into()])["content"][1]["content"][0]["text"],
        "b"
    );
}
