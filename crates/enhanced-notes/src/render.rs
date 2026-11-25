//! TipTap JSON rendering. The user's own nodes are cloned verbatim; AI content
//! is only ever inserted as additional nodes (a nested bullet list under each
//! anchor, and trailing Decisions / Action items / Open questions sections).

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Value, json};

use crate::anchors::{Anchor, children, is_list, is_list_item, line_text, node_type};
use crate::segments::SourceRef;

pub const GROUP_DECISIONS: &str = "@decisions";
pub const GROUP_ACTIONS: &str = "@actions";
pub const GROUP_QUESTIONS: &str = "@questions";

pub const HEADING_DECISIONS: &str = "Decisions";
pub const HEADING_ACTIONS: &str = "Action items";
pub const HEADING_QUESTIONS: &str = "Open questions";
pub const HEADING_OVERVIEW: &str = "Overview";

/// One line under a group: either freshly generated (`node` is None), or an
/// existing node that is preserved (user-typed line, or a user-edited AI line).
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub ai_id: Option<String>,
    pub text: String,
    pub src: Vec<SourceRef>,
    pub node: Option<Value>,
}

/// group key (anchor key or `@decisions` ...) -> ordered lines.
pub type Layout = BTreeMap<String, Vec<Line>>;

pub fn section_heading(group: &str) -> Option<&'static str> {
    match group {
        GROUP_DECISIONS => Some(HEADING_DECISIONS),
        GROUP_ACTIONS => Some(HEADING_ACTIONS),
        GROUP_QUESTIONS => Some(HEADING_QUESTIONS),
        _ => None,
    }
}

pub fn section_group(heading: &str) -> Option<&'static str> {
    match heading.trim().to_lowercase().as_str() {
        "decisions" => Some(GROUP_DECISIONS),
        "action items" => Some(GROUP_ACTIONS),
        "open questions" => Some(GROUP_QUESTIONS),
        _ => None,
    }
}

/// Compact `src` attribute: `[{"t":transcript,"s":start_word,"e":end_word,"a":start_ms,"b":end_ms}]`.
pub fn src_json(src: &[SourceRef]) -> Value {
    Value::Array(
        src.iter()
            .map(|r| {
                json!({
                    "t": r.transcript_id,
                    "s": r.start_word_id,
                    "e": r.end_word_id,
                    "a": r.start_ms,
                    "b": r.end_ms,
                })
            })
            .collect(),
    )
}

pub fn parse_src(value: &Value) -> Vec<SourceRef> {
    let Some(items) = value.as_array() else {
        return vec![];
    };
    items
        .iter()
        .filter_map(|v| {
            Some(SourceRef {
                transcript_id: v.get("t")?.as_str()?.to_owned(),
                start_word_id: v.get("s")?.as_str()?.to_owned(),
                end_word_id: v.get("e")?.as_str()?.to_owned(),
                start_ms: v.get("a")?.as_i64()?,
                end_ms: v.get("b")?.as_i64()?,
            })
        })
        .collect()
}

fn paragraph(text: &str) -> Value {
    if text.is_empty() {
        json!({"type": "paragraph"})
    } else {
        json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
    }
}

fn item_attrs(
    item_type: &str,
    ai_id: Option<&str>,
    src: &[SourceRef],
    old: Option<&Value>,
) -> Value {
    let mut attrs = Map::new();
    if item_type == "taskItem" {
        attrs.insert(
            "checked".into(),
            old.and_then(|o| o.get("checked"))
                .cloned()
                .unwrap_or(json!(false)),
        );
        for key in ["status", "taskId", "taskItemId"] {
            if let Some(v) = old.and_then(|o| o.get(key)) {
                attrs.insert(key.into(), v.clone());
            }
        }
    }
    if let Some(id) = ai_id {
        attrs.insert("aiId".into(), json!(id));
        attrs.insert("src".into(), src_json(src));
    }
    Value::Object(attrs)
}

/// Build the list item for a line, converting to `item_type` (`listItem` / `taskItem`).
pub fn line_node(line: &Line, item_type: &str) -> Value {
    let Some(node) = &line.node else {
        return json!({
            "type": item_type,
            "attrs": item_attrs(item_type, line.ai_id.as_deref(), &line.src, None),
            "content": [paragraph(&line.text)],
        });
    };
    if is_list_item(node) {
        let mut out = node.clone();
        out["type"] = json!(item_type);
        let old = node.get("attrs");
        let ai = line.ai_id.as_deref();
        out["attrs"] = item_attrs(item_type, ai, &line.src, old);
        return out;
    }
    let content = if node_type(node) == "paragraph" {
        node.clone()
    } else {
        paragraph(&line_text(node))
    };
    json!({
        "type": item_type,
        "attrs": item_attrs(item_type, line.ai_id.as_deref(), &line.src, None),
        "content": [content],
    })
}

fn list_node(lines: &[Line], list_type: &str, item_type: &str) -> Value {
    json!({
        "type": list_type,
        "content": lines.iter().map(|l| line_node(l, item_type)).collect::<Vec<_>>(),
    })
}

struct Ctx<'a> {
    by_path: HashMap<&'a [usize], &'a Anchor>,
    layout: &'a Layout,
}

impl Ctx<'_> {
    fn ai_list_at(&self, path: &[usize]) -> Option<Value> {
        let anchor = self.by_path.get(path)?;
        let lines = self.layout.get(&anchor.key)?;
        (!lines.is_empty()).then(|| list_node(lines, "bulletList", "listItem"))
    }

    fn render_children(&self, nodes: &[Value], path: &mut Vec<usize>) -> Vec<Value> {
        let mut out = Vec::new();
        for (index, node) in nodes.iter().enumerate() {
            path.push(index);
            self.render_node(node, path, &mut out);
            path.pop();
        }
        out
    }

    fn render_node(&self, node: &Value, path: &mut Vec<usize>, out: &mut Vec<Value>) {
        if is_list(node) {
            let mut list = node.clone();
            let items: Vec<Value> = children(node)
                .iter()
                .enumerate()
                .map(|(j, item)| {
                    path.push(j);
                    let rendered = self.render_item(item, path);
                    path.pop();
                    rendered
                })
                .collect();
            list["content"] = Value::Array(items);
            out.push(list);
            return;
        }
        if node_type(node) == "blockquote" {
            let mut quote = node.clone();
            quote["content"] = Value::Array(self.render_children(children(node), path));
            out.push(quote);
            return;
        }
        out.push(node.clone());
        if let Some(list) = self.ai_list_at(path) {
            out.push(list);
        }
    }

    fn render_item(&self, item: &Value, path: &mut Vec<usize>) -> Value {
        if !is_list_item(item) {
            return item.clone();
        }
        let mut content: Vec<Value> = Vec::new();
        let kids = children(item);
        if let Some(first) = kids.first() {
            content.push(first.clone());
        }
        if let Some(list) = self.ai_list_at(path) {
            content.push(list);
        }
        for (k, child) in kids.iter().enumerate().skip(1) {
            path.push(k);
            self.render_node(child, path, &mut content);
            path.pop();
        }
        let mut out = item.clone();
        out["content"] = Value::Array(content);
        out
    }
}

/// Render the enhanced document: the raw notes verbatim plus AI lines.
pub fn render_doc(raw_doc: &Value, anchors: &[Anchor], layout: &Layout) -> Value {
    let ctx = Ctx {
        by_path: anchors
            .iter()
            .filter(|a| !a.synthetic)
            .map(|a| (a.path.as_slice(), a))
            .collect(),
        layout,
    };
    let mut content = ctx.render_children(children(raw_doc), &mut Vec::new());

    if anchors.iter().any(|a| a.synthetic)
        && let Some(lines) = layout
            .get(crate::anchors::OVERVIEW_KEY)
            .filter(|l| !l.is_empty())
    {
        content.push(json!({"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": HEADING_OVERVIEW}]}));
        content.push(list_node(lines, "bulletList", "listItem"));
    }

    for (group, list_type, item_type) in [
        (GROUP_DECISIONS, "bulletList", "listItem"),
        (GROUP_ACTIONS, "taskList", "taskItem"),
        (GROUP_QUESTIONS, "bulletList", "listItem"),
    ] {
        let Some(lines) = layout.get(group).filter(|l| !l.is_empty()) else {
            continue;
        };
        let heading = section_heading(group).unwrap_or("");
        content.push(json!({"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": heading}]}));
        content.push(list_node(lines, list_type, item_type));
    }

    if content.is_empty() {
        content.push(json!({"type": "paragraph"}));
    }
    json!({"type": "doc", "content": content})
}
