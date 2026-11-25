mod common;

use common::*;
use enhanced_notes::anchors::{children, extract_anchors};
use enhanced_notes::render::parse_src;
use enhanced_notes::{EnhanceError, EnhanceOutput, enhance};
use serde_json::{Value, json};

#[tokio::test]
async fn skeleton_is_verbatim_and_every_ai_line_has_valid_sources() {
    let raw = raw_notes();
    let result = enhance(input(raw.clone()), &Scripted(scripted_output()))
        .await
        .unwrap();

    // Every raw line appears unchanged and in order, before any generated section.
    let expected = raw_lines(&raw);
    let got = user_lines(&result.doc);
    assert_eq!(&got[..expected.len()], expected.as_slice());
    // The user's own nodes are present untouched (verbatim JSON).
    for a in extract_anchors(&raw) {
        let json = a.block_json.to_string();
        let first_p = children(&a.block_json).first().cloned();
        let doc_text = result.doc.to_string();
        assert!(
            doc_text.contains(&json) || first_p.is_some_and(|p| doc_text.contains(&p.to_string())),
            "anchor `{}` lost",
            a.text
        );
    }

    let words: Vec<String> = transcript().words.iter().map(|w| w.id.clone()).collect();
    let ai = ai_items(&result.doc);
    assert!(ai.len() >= 6);
    for (id, text, node) in &ai {
        let src = parse_src(&node["attrs"]["src"]);
        assert!(!src.is_empty(), "`{text}` has no source");
        for s in src {
            assert!(words.contains(&s.start_word_id) && words.contains(&s.end_word_id));
            assert_eq!(s.transcript_id, "tr1");
        }
        assert!(result.metadata.ai_lines.contains_key(id));
    }
    assert!(anlg_tiptap::validate_tiptap_json(&result.doc).is_empty());
}

#[tokio::test]
async fn sections_have_owner_and_due_and_correct_sources() {
    let result = enhance(input(raw_notes()), &Scripted(scripted_output()))
        .await
        .unwrap();
    let ai = ai_items(&result.doc);
    let action = ai
        .iter()
        .find(|(_, t, _)| t.starts_with("Send the revised pricing"))
        .unwrap();
    assert_eq!(
        action.1,
        "Send the revised pricing to Dana (owner: Alex Kim; due: Friday)"
    );
    assert_eq!(action.2["type"], "taskItem");
    let src = parse_src(&action.2["attrs"]["src"]);
    // s4 = "Alex will send ..." words
    let seg_words: Vec<_> = transcript()
        .words
        .into_iter()
        .filter(|w| w.text.trim() == "Alex")
        .collect();
    assert_eq!(src[0].start_word_id, seg_words[0].id);
    let headings: Vec<_> = result.doc["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"] == "heading")
        .map(enhanced_notes::anchors::line_text)
        .collect();
    assert_eq!(
        headings,
        vec!["Risks", "Decisions", "Action items", "Open questions"]
    );
}

#[tokio::test]
async fn model_garbage_is_filtered_out() {
    let mut out = scripted_output();
    out.sections[0]
        .bullets
        .push(bullet("Invented fact", &["s77"]));
    out.sections[0].bullets.push(bullet("Unsourced", &[]));
    out.sections.push(enhanced_notes::schema::Section {
        anchor: "a42".into(),
        bullets: vec![bullet("ghost", &["s1"])],
    });
    let result = enhance(input(raw_notes()), &Scripted(out)).await.unwrap();
    let texts: Vec<_> = ai_items(&result.doc)
        .into_iter()
        .map(|(_, t, _)| t)
        .collect();
    assert!(
        !texts
            .iter()
            .any(|t| t == "Invented fact" || t == "Unsourced" || t == "ghost")
    );
    assert!(result.report.dropped_bullets >= 3);
}

#[tokio::test]
async fn empty_transcript_is_an_error_and_empty_notes_get_an_overview() {
    let mut i = input(raw_notes());
    i.transcripts = vec![];
    assert!(matches!(
        enhance(i, &Scripted(scripted_output())).await,
        Err(EnhanceError::NoTranscript)
    ));

    let empty = json!({"type":"doc","content":[{"type":"paragraph"}]});
    let out = EnhanceOutput {
        sections: vec![enhanced_notes::schema::Section {
            anchor: "a1".into(),
            bullets: vec![bullet("Team agreed on launch date", &["s2"])],
        }],
        ..Default::default()
    };
    let result = enhance(input(empty), &Scripted(out)).await.unwrap();
    assert_eq!(result.doc["content"][1]["content"][0]["text"], "Overview");
    assert_eq!(ai_items(&result.doc).len(), 1);
}

// ---- reconcile -------------------------------------------------------------

async fn first_run() -> enhanced_notes::EnhanceResult {
    enhance(input(raw_notes()), &Scripted(scripted_output()))
        .await
        .unwrap()
}

fn rerun(
    prev: &enhanced_notes::EnhanceResult,
    raw: Value,
    out: EnhanceOutput,
    regenerate: bool,
    doc: Value,
) -> impl std::future::Future<Output = enhanced_notes::EnhanceResult> {
    let mut i = input(raw);
    i.prev_doc = Some(doc);
    i.prev_metadata = Some(prev.metadata.clone());
    i.regenerate = regenerate;
    async move { enhance(i, &Scripted(out)).await.unwrap() }
}

#[tokio::test]
async fn unchanged_ai_lines_are_replaced_by_fresh_output() {
    let first = first_run().await;
    let mut out = scripted_output();
    out.sections[1].bullets = vec![bullet("Starter plan is $39 per seat", &["s3"])];
    let second = rerun(&first, raw_notes(), out, false, first.doc.clone()).await;
    let texts: Vec<_> = ai_items(&second.doc)
        .into_iter()
        .map(|(_, t, _)| t)
        .collect();
    assert!(texts.contains(&"Starter plan is $39 per seat".to_string()));
    assert!(!texts.contains(&"Starter plan is $29 per seat".to_string()));
}

#[tokio::test]
async fn user_edit_to_an_ai_line_is_kept_and_not_duplicated() {
    let first = first_run().await;
    let mut doc = first.doc.clone();
    edit_ai_text(
        &mut doc,
        "Starter plan is $29 per seat",
        "Starter plan: $29/seat, annual only",
    );
    let second = rerun(&first, raw_notes(), scripted_output(), false, doc).await;
    let texts: Vec<_> = ai_items(&second.doc)
        .into_iter()
        .map(|(_, t, _)| t)
        .collect();
    assert!(texts.contains(&"Starter plan: $29/seat, annual only".to_string()));
    assert!(
        !texts.contains(&"Starter plan is $29 per seat".to_string()),
        "near-duplicate must be suppressed"
    );
    assert_eq!(second.stats.kept_edited, 1);
    // Same aiId survives.
    let id_before = ai_items(&first.doc)
        .into_iter()
        .find(|(_, t, _)| t.starts_with("Starter"))
        .unwrap()
        .0;
    assert!(
        ai_items(&second.doc)
            .iter()
            .any(|(id, _, _)| *id == id_before)
    );
}

#[tokio::test]
async fn user_deleted_ai_line_stays_deleted_until_regenerate() {
    let first = first_run().await;
    let mut doc = first.doc.clone();
    let id = ai_items(&doc)
        .into_iter()
        .find(|(_, t, _)| t == "Two backend engineers approved")
        .unwrap()
        .0;
    delete_ai_item(&mut doc, "Two backend engineers approved");
    let second = rerun(&first, raw_notes(), scripted_output(), false, doc.clone()).await;
    assert!(
        !ai_items(&second.doc)
            .iter()
            .any(|(_, t, _)| t == "Two backend engineers approved")
    );
    assert!(second.metadata.deleted.contains(&id));

    // A third pass over the second doc still honors the delete (tombstones persist).
    let third = rerun(
        &second,
        raw_notes(),
        scripted_output(),
        false,
        second.doc.clone(),
    )
    .await;
    assert!(
        !ai_items(&third.doc)
            .iter()
            .any(|(_, t, _)| t == "Two backend engineers approved")
    );

    // Regenerate resets edits and deletions.
    let mut edited = second.doc.clone();
    edit_ai_text(&mut edited, "Starter plan is $29 per seat", "my edit");
    let regen = rerun(&second, raw_notes(), scripted_output(), true, edited).await;
    let texts: Vec<_> = ai_items(&regen.doc)
        .into_iter()
        .map(|(_, t, _)| t)
        .collect();
    assert!(texts.contains(&"Two backend engineers approved".to_string()));
    assert!(texts.contains(&"Starter plan is $29 per seat".to_string()));
    assert!(!texts.contains(&"my edit".to_string()));
    assert!(regen.metadata.deleted.is_empty());
}

#[tokio::test]
async fn lines_typed_into_the_enhanced_doc_stay_under_their_anchor() {
    let first = first_run().await;
    let mut doc = first.doc.clone();
    add_user_item_next_to(
        &mut doc,
        "Starter plan is $29 per seat",
        "ask finance about annual billing",
    );
    let second = rerun(&first, raw_notes(), scripted_output(), false, doc).await;
    let lines = user_lines(&second.doc);
    let pricing = lines.iter().position(|l| l == "Pricing").unwrap();
    let hiring = lines.iter().position(|l| l == "Hiring").unwrap();
    let typed = lines
        .iter()
        .position(|l| l == "ask finance about annual billing")
        .expect("typed line kept");
    assert!(
        pricing < typed && typed < hiring,
        "typed line sits under Pricing: {lines:?}"
    );
    assert_eq!(second.stats.kept_user, 1);
}

#[tokio::test]
async fn removed_anchor_drops_unedited_ai_lines_and_rehomes_edits() {
    let first = first_run().await;
    let mut doc = first.doc.clone();
    edit_ai_text(
        &mut doc,
        "Two backend engineers approved",
        "Two backend engineers approved (Dana to confirm)",
    );
    // The user removes "Hiring" (and its nested bullet) from the raw notes.
    let raw = json!({"type":"doc","content":[
        p("Q3 launch plan"),
        list(vec![item("Pricing")]),
        heading("Risks"),
    ]});
    let out = EnhanceOutput {
        sections: scripted_output().sections[..2].to_vec(),
        ..scripted_output()
    };
    let second = rerun(&first, raw.clone(), out, false, doc).await;
    let texts: Vec<_> = ai_items(&second.doc)
        .into_iter()
        .map(|(_, t, _)| t)
        .collect();
    assert!(!user_lines(&second.doc).contains(&"Hiring".to_string()));
    // The edited line survives (moved under the nearest earlier anchor); nothing unedited remains.
    assert!(texts.contains(&"Two backend engineers approved (Dana to confirm)".to_string()));
    assert_eq!(
        &user_lines(&second.doc)[..raw_lines(&raw).len()],
        raw_lines(&raw).as_slice()
    );

    // Unedited case: plain removal drops its AI lines.
    let third = rerun(
        &first,
        raw,
        EnhanceOutput {
            sections: scripted_output().sections[..2].to_vec(),
            ..scripted_output()
        },
        false,
        first.doc.clone(),
    )
    .await;
    let texts: Vec<_> = ai_items(&third.doc)
        .into_iter()
        .map(|(_, t, _)| t)
        .collect();
    assert!(!texts.contains(&"Two backend engineers approved".to_string()));
}

#[tokio::test]
async fn lightly_edited_anchor_keeps_its_ai_lines() {
    let first = first_run().await;
    let mut doc = first.doc.clone();
    edit_ai_text(
        &mut doc,
        "Starter plan is $29 per seat",
        "Starter plan $29/seat (annual)",
    );
    let raw = json!({"type":"doc","content":[
        p("Q3 launch plan"),
        list(vec![item("Pricing plans"), item_with("Hiring", vec![list(vec![item("Backend engineer")])])]),
        heading("Risks"),
    ]});
    let mut out = scripted_output();
    out.sections[1].anchor = "a2".into();
    let second = rerun(&first, raw, out, false, doc).await;
    let lines = user_lines(&second.doc);
    assert!(lines.contains(&"Pricing plans".to_string()));
    let ai = ai_items(&second.doc);
    assert!(
        ai.iter()
            .any(|(_, t, _)| t == "Starter plan $29/seat (annual)"),
        "edit kept across anchor rename"
    );
    assert_eq!(second.stats.kept_edited, 1);
}

#[tokio::test]
async fn result_round_trips_through_markdown_conversion() {
    let result = first_run().await;
    let md = anlg_tiptap::tiptap_json_to_md(&result.doc).unwrap();
    assert!(md.contains("Pricing") && md.contains("Starter plan is"));
}
