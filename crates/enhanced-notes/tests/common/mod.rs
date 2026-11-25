#![allow(dead_code)]

use enhanced_notes::schema::{ActionItem, Bullet, Section};
use enhanced_notes::{
    EnhanceError, EnhanceInput, EnhanceOptions, EnhanceOutput, ExtractClient, SpeakerLabels,
    TranscriptInput, Word, anchors::extract_anchors,
};
use serde_json::{Value, json};

pub fn p(text: &str) -> Value {
    json!({"type":"paragraph","content":[{"type":"text","text":text}]})
}

pub fn item(text: &str) -> Value {
    json!({"type":"listItem","content":[p(text)]})
}

pub fn item_with(text: &str, nested: Vec<Value>) -> Value {
    let mut content = vec![p(text)];
    content.extend(nested);
    json!({"type":"listItem","content":content})
}

pub fn list(items: Vec<Value>) -> Value {
    json!({"type":"bulletList","content":items})
}

pub fn heading(text: &str) -> Value {
    json!({"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":text}]})
}

/// Raw notes: a paragraph, a bullet list with a nested bullet, and a heading.
pub fn raw_notes() -> Value {
    json!({"type":"doc","content":[
        p("Q3 launch plan"),
        list(vec![
            item("Pricing"),
            item_with("Hiring", vec![list(vec![item("Backend engineer")])]),
        ]),
        heading("Risks"),
    ]})
}

/// (channel, speaker_index, sentence). Each sentence becomes one segment
/// (5 s of silence between turns).
pub const MEETING: &[(i32, Option<i32>, &str)] = &[
    (0, None, "Let's start with the Q3 launch plan."),
    (1, Some(0), "We decided to launch on November 14."),
    (
        0,
        None,
        "On pricing the starter plan is twenty nine dollars per seat.",
    ),
    (
        1,
        Some(0),
        "Alex will send the revised pricing to Dana by Friday.",
    ),
    (0, None, "For hiring we approved two backend engineers."),
    (1, Some(0), "Who owns the SOC2 audit is still open."),
];

pub fn transcript() -> TranscriptInput {
    let mut words = Vec::new();
    let mut t = 0i64;
    let mut n = 0;
    for (channel, speaker, sentence) in MEETING {
        for w in sentence.split_whitespace() {
            n += 1;
            words.push(Word {
                id: format!("w{n}"),
                text: format!(" {w}"),
                start_ms: t,
                end_ms: t + 300,
                channel: *channel,
                speaker_index: *speaker,
            });
            t += 300;
        }
        t += 5000;
    }
    TranscriptInput {
        transcript_id: "tr1".into(),
        words,
    }
}

pub fn bullet(text: &str, sources: &[&str]) -> Bullet {
    Bullet {
        text: text.into(),
        sources: sources.iter().map(|s| s.to_string()).collect(),
    }
}

/// What a well-behaved model would answer for MEETING / raw_notes.
pub fn scripted_output() -> EnhanceOutput {
    EnhanceOutput {
        sections: vec![
            Section {
                anchor: "a1".into(),
                bullets: vec![bullet("Launch planning kicked off for Q3", &["s1"])],
            },
            Section {
                anchor: "a2".into(),
                bullets: vec![bullet("Starter plan is $29 per seat", &["s3"])],
            },
            Section {
                anchor: "a3".into(),
                bullets: vec![bullet("Two backend engineers approved", &["s5"])],
            },
        ],
        decisions: vec![bullet("Launch on November 14", &["s2"])],
        action_items: vec![ActionItem {
            text: "Send the revised pricing to Dana".into(),
            owner: Some("Alex".into()),
            due: Some("Friday".into()),
            sources: vec!["s4".into()],
        }],
        open_questions: vec![bullet("Who owns the SOC2 audit?", &["s6"])],
    }
}

pub struct Scripted(pub EnhanceOutput);

impl ExtractClient for Scripted {
    fn model_name(&self) -> String {
        "scripted".into()
    }
    async fn extract(
        &self,
        _s: &str,
        _u: &str,
        _schema: &Value,
    ) -> Result<EnhanceOutput, EnhanceError> {
        Ok(self.0.clone())
    }
}

pub fn input(raw: Value) -> EnhanceInput {
    EnhanceInput {
        raw_doc: raw,
        transcripts: vec![transcript()],
        speaker_labels: SpeakerLabels::default(),
        participants: vec!["Alex Kim".into(), "Dana Fox".into()],
        prev_doc: None,
        prev_metadata: None,
        regenerate: false,
        options: EnhanceOptions::default(),
    }
}

/// All AI list items (attrs.aiId present), in document order, as (id, text, node).
pub fn ai_items(doc: &Value) -> Vec<(String, String, Value)> {
    fn walk(node: &Value, out: &mut Vec<(String, String, Value)>) {
        if let Some(id) = node
            .get("attrs")
            .and_then(|a| a.get("aiId"))
            .and_then(Value::as_str)
        {
            out.push((
                id.to_owned(),
                enhanced_notes::anchors::line_text(node),
                node.clone(),
            ));
        }
        for child in enhanced_notes::anchors::children(node) {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    walk(doc, &mut out);
    out
}

/// Text of every non-AI line (paragraph, heading, list item) in document order.
pub fn user_lines(doc: &Value) -> Vec<String> {
    use enhanced_notes::anchors::{children, is_list_item, line_text, node_type};
    fn walk(node: &Value, out: &mut Vec<String>) {
        let ai = node.get("attrs").and_then(|a| a.get("aiId")).is_some();
        if is_list_item(node) {
            if !ai {
                let t = line_text(node);
                if !t.is_empty() {
                    out.push(t);
                }
            }
            if ai {
                return;
            }
            for child in children(node).iter().skip(1) {
                walk(child, out);
            }
            return;
        }
        if matches!(node_type(node), "paragraph" | "heading") {
            let t = line_text(node);
            if !t.is_empty() {
                out.push(t);
            }
            return;
        }
        for child in children(node) {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    walk(doc, &mut out);
    out
}

pub fn raw_lines(raw: &Value) -> Vec<String> {
    extract_anchors(raw).into_iter().map(|a| a.text).collect()
}

/// Replace the text of the first AI item whose text equals `from`.
pub fn edit_ai_text(doc: &mut Value, from: &str, to: &str) {
    fn walk(node: &mut Value, from: &str, to: &str) -> bool {
        let is_ai = node.get("attrs").and_then(|a| a.get("aiId")).is_some();
        if is_ai && enhanced_notes::anchors::line_text(node) == from {
            node["content"][0]["content"][0]["text"] = json!(to);
            return true;
        }
        if let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) {
            for c in children {
                if walk(c, from, to) {
                    return true;
                }
            }
        }
        false
    }
    assert!(walk(doc, from, to), "AI line `{from}` not found");
}

/// Remove the AI item with the given text from its list (dropping an emptied list).
pub fn delete_ai_item(doc: &mut Value, text: &str) {
    fn walk(node: &mut Value, text: &str) -> bool {
        let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) else {
            return false;
        };
        if let Some(pos) = children.iter().position(|c| {
            c.get("attrs").and_then(|a| a.get("aiId")).is_some()
                && enhanced_notes::anchors::line_text(c) == text
        }) {
            children.remove(pos);
            return true;
        }
        for c in children.iter_mut() {
            if walk(c, text) {
                return true;
            }
        }
        false
    }
    assert!(walk(doc, text), "AI line `{text}` not found");
    // drop emptied lists
    fn prune(node: &mut Value) {
        if let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) {
            for c in children.iter_mut() {
                prune(c);
            }
            children.retain(|c| {
                !(matches!(c["type"].as_str(), Some("bulletList" | "taskList"))
                    && c.get("content")
                        .and_then(Value::as_array)
                        .is_some_and(Vec::is_empty))
            });
        }
    }
    prune(doc);
}

/// Append a plain user-typed item to the AI list that contains `sibling_text`.
pub fn add_user_item_next_to(doc: &mut Value, sibling_text: &str, typed: &str) {
    fn walk(node: &mut Value, sibling: &str, typed: &str) -> bool {
        let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) else {
            return false;
        };
        if children.iter().any(|c| {
            c.get("attrs").and_then(|a| a.get("aiId")).is_some()
                && enhanced_notes::anchors::line_text(c) == sibling
        }) {
            children.push(item(typed));
            return true;
        }
        for c in children.iter_mut() {
            if walk(c, sibling, typed) {
                return true;
            }
        }
        false
    }
    assert!(walk(doc, sibling_text, typed));
}
