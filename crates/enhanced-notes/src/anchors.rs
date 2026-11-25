//! Anchors: the user's own typed lines. Each non-empty top-level block and each
//! typed list item (at any depth) is an anchor. Anchors are never rewritten;
//! the model only references them.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::util::{normalize, sha_hex, similarity};

/// Key used for the synthetic anchor when the user typed nothing.
pub const OVERVIEW_KEY: &str = "@overview";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    /// Stable id: hash of the normalized text plus the occurrence index among
    /// identical lines, e.g. `3fa9c2b1d4-0`.
    pub key: String,
    pub ordinal: usize,
    /// The node exactly as typed (a block, or a list item).
    pub block_json: Value,
    pub text: String,
    /// Child-index path from the document root to `block_json`.
    pub path: Vec<usize>,
    /// True for the generated "Overview" anchor used when there are no notes.
    #[serde(default)]
    pub synthetic: bool,
}

impl Anchor {
    /// Short id shown to the model (`a1`, `a2`, ...).
    pub fn prompt_id(&self) -> String {
        format!("a{}", self.ordinal + 1)
    }
}

/// Minimal anchor record persisted in generation metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorRef {
    pub key: String,
    pub text: String,
}

pub fn node_type(node: &Value) -> &str {
    node.get("type").and_then(Value::as_str).unwrap_or("")
}

pub fn children(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

pub fn is_list(node: &Value) -> bool {
    matches!(node_type(node), "bulletList" | "orderedList" | "taskList")
}

pub fn is_list_item(node: &Value) -> bool {
    matches!(node_type(node), "listItem" | "taskItem")
}

/// Plain text of an inline-bearing node (hard breaks become spaces; mentions use their label).
pub fn inline_text(node: &Value) -> String {
    if let Some(text) = node.get("text").and_then(Value::as_str) {
        return text.to_owned();
    }
    match node_type(node) {
        "hardBreak" => return " ".to_owned(),
        t if t.starts_with("mention-") => {
            let attrs = node.get("attrs");
            for key in ["label", "name", "id"] {
                if let Some(value) = attrs.and_then(|a| a.get(key)).and_then(Value::as_str) {
                    return value.to_owned();
                }
            }
            return String::new();
        }
        _ => {}
    }
    children(node).iter().map(inline_text).collect()
}

/// Text of a node's own line: for list items, the first paragraph only.
pub fn line_text(node: &Value) -> String {
    let raw = if is_list_item(node) {
        children(node)
            .first()
            .filter(|first| node_type(first) == "paragraph")
            .map(inline_text)
            .unwrap_or_default()
    } else {
        inline_text(node)
    };
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct RawAnchor {
    node: Value,
    text: String,
    path: Vec<usize>,
}

fn walk_children(nodes: &[Value], path: &mut Vec<usize>, out: &mut Vec<RawAnchor>) {
    for (index, node) in nodes.iter().enumerate() {
        path.push(index);
        walk_node(node, path, out);
        path.pop();
    }
}

/// `path` already ends with the node's own index.
fn walk_node(node: &Value, path: &mut Vec<usize>, out: &mut Vec<RawAnchor>) {
    match node_type(node) {
        "bulletList" | "orderedList" | "taskList" => {
            for (item_index, item) in children(node).iter().enumerate() {
                if !is_list_item(item) {
                    continue;
                }
                path.push(item_index);
                let text = line_text(item);
                if !text.is_empty() {
                    out.push(RawAnchor {
                        node: item.clone(),
                        text,
                        path: path.clone(),
                    });
                }
                // Only the blocks after the item's own first paragraph.
                for (child_index, child) in children(item).iter().enumerate().skip(1) {
                    path.push(child_index);
                    walk_node(child, path, out);
                    path.pop();
                }
                path.pop();
            }
        }
        "blockquote" => walk_children(children(node), path, out),
        "paragraph" | "heading" => {
            let text = line_text(node);
            if !text.is_empty() {
                out.push(RawAnchor {
                    node: node.clone(),
                    text,
                    path: path.clone(),
                });
            }
        }
        _ => {}
    }
}

/// Extract anchors from a raw TipTap document.
pub fn extract_anchors(doc: &Value) -> Vec<Anchor> {
    let mut raw = Vec::new();
    walk_children(children(doc), &mut Vec::new(), &mut raw);
    let mut seen: HashMap<String, usize> = HashMap::new();
    raw.into_iter()
        .enumerate()
        .map(|(ordinal, anchor)| {
            let norm = normalize(&anchor.text);
            let occurrence = seen.entry(norm.clone()).or_insert(0);
            let key = format!("{}-{}", sha_hex(&norm, 10), occurrence);
            *occurrence += 1;
            Anchor {
                key,
                ordinal,
                block_json: anchor.node,
                text: anchor.text,
                path: anchor.path,
                synthetic: false,
            }
        })
        .collect()
}

/// The anchor used when the user typed no notes at all.
pub fn overview_anchor() -> Anchor {
    Anchor {
        key: OVERVIEW_KEY.to_owned(),
        ordinal: 0,
        block_json: Value::Null,
        text: "Meeting overview".to_owned(),
        path: vec![],
        synthetic: true,
    }
}

/// Map anchors of a previous generation onto the current anchors.
/// Exact keys win; lightly edited lines are matched by text similarity and
/// relative position. Each current anchor is used at most once.
pub fn rematch(prev: &[AnchorRef], current: &[Anchor]) -> HashMap<String, String> {
    let mut mapping = HashMap::new();
    let mut used = vec![false; current.len()];
    let mut prev_done = vec![false; prev.len()];

    for (pi, p) in prev.iter().enumerate() {
        if let Some(ci) = current.iter().position(|c| c.key == p.key) {
            mapping.insert(p.key.clone(), current[ci].key.clone());
            used[ci] = true;
            prev_done[pi] = true;
        }
    }

    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (pi, p) in prev.iter().enumerate().filter(|(i, _)| !prev_done[*i]) {
        for (ci, c) in current.iter().enumerate().filter(|(i, _)| !used[*i]) {
            let sim = similarity(&p.text, &c.text);
            if sim < 0.5 {
                continue;
            }
            let pos_prev = pi as f64 / prev.len().max(1) as f64;
            let pos_cur = ci as f64 / current.len().max(1) as f64;
            pairs.push((sim - 0.2 * (pos_prev - pos_cur).abs(), pi, ci));
        }
    }
    pairs.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });
    for (_, pi, ci) in pairs {
        if prev_done[pi] || used[ci] {
            continue;
        }
        prev_done[pi] = true;
        used[ci] = true;
        mapping.insert(prev[pi].key.clone(), current[ci].key.clone());
    }
    mapping
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn p(t: &str) -> Value {
        json!({"type":"paragraph","content":[{"type":"text","text":t}]})
    }
    fn li(t: &str, nested: Vec<Value>) -> Value {
        let mut c = vec![p(t)];
        c.extend(nested);
        json!({"type":"listItem","content":c})
    }

    #[test]
    fn extracts_blocks_and_typed_bullets_in_order() {
        let doc = json!({"type":"doc","content":[
            p("Agenda"),
            p(""),
            {"type":"bulletList","content":[li("One", vec![json!({"type":"bulletList","content":[li("One-a", vec![])]})]), li("Two", vec![])]},
            {"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":"Risks"}]},
            {"type":"horizontalRule"},
        ]});
        let a = extract_anchors(&doc);
        let texts: Vec<_> = a.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, ["Agenda", "One", "One-a", "Two", "Risks"]);
        assert_eq!(a[0].path, vec![0]);
        assert_eq!(a[1].path, vec![2, 0]);
        assert_eq!(a[2].path, vec![2, 0, 1, 0]);
        assert_eq!(a[1].block_json["type"], "listItem");
        assert_eq!(a[3].ordinal, 3);
    }

    #[test]
    fn identical_lines_get_distinct_occurrence_keys_and_keys_ignore_case_and_spacing() {
        let doc = json!({"type":"doc","content":[p("Todo"), p("  todo "), p("Other")]});
        let a = extract_anchors(&doc);
        assert_ne!(a[0].key, a[1].key);
        assert!(a[0].key.ends_with("-0") && a[1].key.ends_with("-1"));
        let again = extract_anchors(&doc);
        assert_eq!(a, again);
        let (b, c) = (
            extract_anchors(&json!({"type":"doc","content":[p("Pricing")]})),
            extract_anchors(&json!({"type":"doc","content":[p("PRICING  ")]})),
        );
        assert_eq!(b[0].key, c[0].key);
    }

    #[test]
    fn rematch_pairs_lightly_edited_lines_by_similarity_and_position() {
        let prev_doc =
            json!({"type":"doc","content":[p("Budget review"), p("Hiring plan"), p("Offsite")]});
        let prev: Vec<AnchorRef> = extract_anchors(&prev_doc)
            .into_iter()
            .map(|a| AnchorRef {
                key: a.key,
                text: a.text,
            })
            .collect();
        let now = extract_anchors(
            &json!({"type":"doc","content":[p("Budget review (Q3)"), p("Hiring plan"), p("Totally new topic")]}),
        );
        let map = rematch(&prev, &now);
        assert_eq!(map.get(&prev[0].key), Some(&now[0].key));
        assert_eq!(map.get(&prev[1].key), Some(&now[1].key));
        assert!(
            !map.contains_key(&prev[2].key),
            "unrelated line is not forced onto a match"
        );
    }
}
