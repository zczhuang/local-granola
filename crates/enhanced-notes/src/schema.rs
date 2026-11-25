//! The structured output the model must produce, its JSON Schema (sent to
//! Ollama as `format`) and `validate`, which enforces the contract in code so
//! that nothing unsupported reaches the document.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::anchors::Anchor;
use crate::segments::PromptSegment;
use crate::util::{normalize, similarity};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bullet {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Section {
    #[serde(default)]
    pub anchor: String,
    #[serde(default)]
    pub bullets: Vec<Bullet>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActionItem {
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default)]
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EnhanceOutput {
    #[serde(default)]
    pub sections: Vec<Section>,
    #[serde(default)]
    pub decisions: Vec<Bullet>,
    #[serde(default)]
    pub action_items: Vec<ActionItem>,
    #[serde(default)]
    pub open_questions: Vec<Bullet>,
}

/// JSON Schema for [`EnhanceOutput`] (draft-07 subset understood by Ollama's
/// structured-output grammar).
pub fn output_json_schema() -> Value {
    let sources = json!({
        "type": "array",
        "minItems": 1,
        "maxItems": 6,
        "items": {"type": "string"}
    });
    let bullet = json!({
        "type": "object",
        "properties": {
            "text": {"type": "string"},
            "sources": sources
        },
        "required": ["text", "sources"],
        "additionalProperties": false
    });
    json!({
        "type": "object",
        "properties": {
            "sections": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "anchor": {"type": "string"},
                        "bullets": {"type": "array", "items": bullet}
                    },
                    "required": ["anchor", "bullets"],
                    "additionalProperties": false
                }
            },
            "decisions": {"type": "array", "items": bullet},
            "action_items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "text": {"type": "string"},
                        "owner": {"type": "string"},
                        "due": {"type": "string"},
                        "sources": sources
                    },
                    "required": ["text", "sources"],
                    "additionalProperties": false
                }
            },
            "open_questions": {"type": "array", "items": bullet}
        },
        "required": ["sections", "decisions", "action_items", "open_questions"],
        "additionalProperties": false
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationReport {
    pub unknown_anchors: usize,
    pub dropped_bullets: usize,
    pub unknown_sources: usize,
}

fn sid_number(sid: &str) -> Option<usize> {
    sid.trim().strip_prefix('s')?.parse().ok()
}

fn clean_sources(
    raw: &[String],
    known: &HashSet<&str>,
    report: &mut ValidationReport,
) -> Vec<String> {
    let mut nums: Vec<usize> = Vec::new();
    for s in raw {
        // Accept `s3`, `S3`, `[s3]` and `3`-less variants only if they name a known sid.
        let cleaned = s.trim().trim_matches(['[', ']']).to_lowercase();
        if known.contains(cleaned.as_str()) {
            if let Some(n) = sid_number(&cleaned)
                && !nums.contains(&n)
            {
                nums.push(n);
            }
        } else {
            report.unknown_sources += 1;
        }
    }
    nums.sort_unstable();
    nums.into_iter().map(|n| format!("s{n}")).collect()
}

fn clean_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn resolve_owner(
    owner: Option<&str>,
    participants: &[String],
    cited: &[&PromptSegment],
) -> Option<String> {
    let owner = clean_text(owner?);
    if owner.is_empty()
        || matches!(
            normalize(&owner).as_str(),
            "unknown" | "n/a" | "none" | "null"
        )
    {
        return None;
    }
    let norm = normalize(&owner);
    if let Some(p) = participants.iter().find(|p| normalize(p) == norm) {
        return Some(p.clone());
    }
    // Unique first-name / prefix match against the participant list.
    let matches: Vec<&String> = participants
        .iter()
        .filter(|p| {
            let pn = normalize(p);
            pn.split(' ').next() == Some(norm.as_str())
                || norm.split(' ').next() == pn.split(' ').next()
        })
        .collect();
    if matches.len() == 1 {
        return Some(matches[0].clone());
    }
    if participants.is_empty() {
        return Some(owner);
    }
    // Not a known participant: keep only if the name is actually spoken or is a speaker label.
    let spoken = cited
        .iter()
        .any(|s| normalize(&s.text).contains(&norm) || normalize(&s.speaker) == norm);
    spoken.then_some(owner)
}

/// Enforce the contract: drop unknown anchors and segment ids, bullets without
/// a valid source and empty text; resolve anchors to their stable keys; merge
/// duplicate sections and exact duplicates; normalize owners.
pub fn validate(
    output: EnhanceOutput,
    anchors: &[Anchor],
    segments: &[PromptSegment],
    participants: &[String],
) -> (EnhanceOutput, ValidationReport) {
    let mut report = ValidationReport::default();
    let known: HashSet<&str> = segments.iter().map(|s| s.sid.as_str()).collect();
    let seg_by_sid: BTreeMap<&str, &PromptSegment> =
        segments.iter().map(|s| (s.sid.as_str(), s)).collect();
    let anchor_by_prompt_id: BTreeMap<String, &Anchor> =
        anchors.iter().map(|a| (a.prompt_id(), a)).collect();

    let mut sections: Vec<Section> = Vec::new();
    for section in output.sections {
        let id = section
            .anchor
            .trim()
            .trim_matches(['[', ']'])
            .to_lowercase();
        let by_key = anchors.iter().find(|a| a.key == section.anchor.trim());
        let Some(anchor) = anchor_by_prompt_id.get(&id).copied().or(by_key) else {
            report.unknown_anchors += 1;
            report.dropped_bullets += section.bullets.len();
            continue;
        };
        let target = match sections.iter().position(|s| s.anchor == anchor.key) {
            Some(i) => i,
            None => {
                sections.push(Section {
                    anchor: anchor.key.clone(),
                    bullets: vec![],
                });
                sections.len() - 1
            }
        };
        for bullet in section.bullets {
            let text = clean_text(&bullet.text);
            let sources = clean_sources(&bullet.sources, &known, &mut report);
            if text.is_empty() || sources.is_empty() {
                report.dropped_bullets += 1;
                continue;
            }
            push_unique(&mut sections[target].bullets, text, sources);
        }
    }
    sections.retain(|s| !s.bullets.is_empty());

    let mut clean_list = |list: Vec<Bullet>| -> Vec<Bullet> {
        let mut out = Vec::new();
        for bullet in list {
            let text = clean_text(&bullet.text);
            let sources = clean_sources(&bullet.sources, &known, &mut report);
            if text.is_empty() || sources.is_empty() {
                report.dropped_bullets += 1;
                continue;
            }
            push_unique(&mut out, text, sources);
        }
        out
    };
    let decisions = clean_list(output.decisions);
    let open_questions = clean_list(output.open_questions);

    let mut action_items: Vec<ActionItem> = Vec::new();
    for item in output.action_items {
        let text = clean_text(&item.text);
        let sources = clean_sources(&item.sources, &known, &mut report);
        if text.is_empty() || sources.is_empty() {
            report.dropped_bullets += 1;
            continue;
        }
        let cited: Vec<&PromptSegment> = sources
            .iter()
            .filter_map(|s| seg_by_sid.get(s.as_str()).copied())
            .collect();
        let owner = resolve_owner(item.owner.as_deref(), participants, &cited);
        let due = item.due.as_deref().map(clean_text).filter(|d| {
            !d.is_empty() && !matches!(normalize(d).as_str(), "none" | "null" | "n/a" | "unknown")
        });
        if let Some(existing) = action_items
            .iter_mut()
            .find(|a| is_duplicate(&a.text, &text))
        {
            union_sources(&mut existing.sources, &sources);
            existing.owner = existing.owner.take().or(owner);
            existing.due = existing.due.take().or(due);
        } else {
            action_items.push(ActionItem {
                text,
                owner,
                due,
                sources,
            });
        }
    }

    (
        EnhanceOutput {
            sections,
            decisions,
            action_items,
            open_questions,
        },
        report,
    )
}

pub(crate) fn is_duplicate(a: &str, b: &str) -> bool {
    normalize(a) == normalize(b) || similarity(a, b) >= 0.85
}

pub(crate) fn union_sources(into: &mut Vec<String>, more: &[String]) {
    for s in more {
        if !into.contains(s) {
            into.push(s.clone());
        }
    }
    into.sort_by_key(|s| sid_number(s).unwrap_or(usize::MAX));
}

fn push_unique(list: &mut Vec<Bullet>, text: String, sources: Vec<String>) {
    if let Some(existing) = list.iter_mut().find(|b| is_duplicate(&b.text, &text)) {
        union_sources(&mut existing.sources, &sources);
    } else {
        list.push(Bullet { text, sources });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchors::extract_anchors;
    use crate::segments::{PromptSegment, SourceRef};

    fn seg(n: usize, speaker: &str, text: &str) -> PromptSegment {
        PromptSegment {
            sid: format!("s{n}"),
            speaker: speaker.into(),
            start_ms: 0,
            text: text.into(),
            source: SourceRef {
                transcript_id: "t".into(),
                start_word_id: format!("w{n}"),
                end_word_id: format!("w{n}"),
                start_ms: 0,
                end_ms: 1,
            },
        }
    }

    fn doc(lines: &[&str]) -> Value {
        json!({"type":"doc","content": lines.iter().map(|l| json!({"type":"paragraph","content":[{"type":"text","text":l}]})).collect::<Vec<_>>()})
    }

    #[test]
    fn schema_is_closed_and_requires_all_lists() {
        let schema = output_json_schema();
        assert_eq!(schema["additionalProperties"], false);
        let required = schema["required"].as_array().unwrap();
        for k in ["sections", "decisions", "action_items", "open_questions"] {
            assert!(required.contains(&json!(k)));
        }
        let item = &schema["properties"]["action_items"]["items"];
        assert_eq!(item["required"], json!(["text", "sources"]));
        assert_eq!(item["properties"]["sources"]["minItems"], 1);
    }

    #[test]
    fn drops_unknown_anchors_sources_and_empty_text() {
        let anchors = extract_anchors(&doc(&["Budget", "Hiring"]));
        let segs = vec![
            seg(1, "Dana", "Budget is 50k"),
            seg(2, "Alex", "I will email Sam"),
        ];
        let out = EnhanceOutput {
            sections: vec![
                Section {
                    anchor: "a1".into(),
                    bullets: vec![
                        Bullet {
                            text: "Budget is 50k".into(),
                            sources: vec!["s1".into(), "s99".into()],
                        },
                        Bullet {
                            text: "Invented".into(),
                            sources: vec!["s42".into()],
                        },
                        Bullet {
                            text: "  ".into(),
                            sources: vec!["s1".into()],
                        },
                        Bullet {
                            text: "No source".into(),
                            sources: vec![],
                        },
                    ],
                },
                Section {
                    anchor: "a9".into(),
                    bullets: vec![Bullet {
                        text: "x".into(),
                        sources: vec!["s1".into()],
                    }],
                },
            ],
            decisions: vec![Bullet {
                text: "Go".into(),
                sources: vec!["s2".into()],
            }],
            action_items: vec![ActionItem {
                text: "Email Sam".into(),
                owner: Some("alex".into()),
                due: Some("none".into()),
                sources: vec!["s2".into()],
            }],
            open_questions: vec![],
        };
        let (v, report) = validate(
            out,
            &anchors,
            &segs,
            &["Alex Kim".into(), "Dana Fox".into()],
        );
        assert_eq!(v.sections.len(), 1);
        assert_eq!(v.sections[0].anchor, anchors[0].key);
        assert_eq!(v.sections[0].bullets.len(), 1);
        assert_eq!(v.sections[0].bullets[0].sources, vec!["s1"]);
        assert_eq!(report.unknown_anchors, 1);
        assert!(report.dropped_bullets >= 3);
        assert_eq!(v.action_items[0].owner.as_deref(), Some("Alex Kim"));
        assert_eq!(v.action_items[0].due, None);
    }

    #[test]
    fn owner_not_in_roster_or_transcript_is_dropped() {
        let anchors = extract_anchors(&doc(&["x"]));
        let segs = vec![seg(1, "Dana", "Please send the deck")];
        let out = EnhanceOutput {
            action_items: vec![ActionItem {
                text: "Send deck".into(),
                owner: Some("Zed".into()),
                due: None,
                sources: vec!["s1".into()],
            }],
            ..Default::default()
        };
        let (v, _) = validate(out, &anchors, &segs, &["Dana Fox".into()]);
        assert_eq!(v.action_items[0].owner, None);
    }

    #[test]
    fn output_parses_with_missing_arrays() {
        let parsed: EnhanceOutput = serde_json::from_str(r#"{"sections":[]}"#).unwrap();
        assert!(parsed.decisions.is_empty());
    }
}
