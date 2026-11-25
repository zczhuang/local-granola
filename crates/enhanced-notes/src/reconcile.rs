//! Re-enhancing without clobbering the user. Compares the previously saved
//! enhanced document (which the user may have edited) with fresh model output:
//!
//! * unchanged AI lines (hash matches what we generated) are replaced by fresh output;
//! * user-edited AI lines are kept, and near-duplicate fresh bullets suppressed;
//! * AI lines the user deleted stay deleted;
//! * lines the user typed into the enhanced document stay under their anchor;
//! * anchors removed from the raw notes drop their unedited AI lines;
//! * `regenerate = true` discards AI edits and deletions.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::anchors::{
    Anchor, AnchorRef, OVERVIEW_KEY, children, is_list_item, line_text, node_type, rematch,
};
use crate::render::{
    GROUP_ACTIONS, GROUP_DECISIONS, GROUP_QUESTIONS, HEADING_OVERVIEW, Layout, Line, parse_src,
    section_group,
};
use crate::schema::{ActionItem, EnhanceOutput, is_duplicate};
use crate::segments::{PromptSegment, SourceRef, compact_sources};
use crate::util::{normalize, sha_hex, similarity};

pub const METADATA_SCHEMA_VERSION: u32 = 1;
const MAX_DELETED: usize = 500;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Engine {
    pub name: String,
    pub version: String,
    pub model: String,
    pub num_ctx: usize,
    pub chunks: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiLineMeta {
    /// Hash of the line text as generated; a different hash in the document means the user edited it.
    pub hash: String,
    /// Group the line belongs to: an anchor key or `@decisions` / `@actions` / `@questions`.
    pub anchor: String,
    /// The generated text (used to suppress re-adding similar bullets after a delete).
    pub text: String,
}

/// `generation_metadata_json` of the enhanced note.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Metadata {
    pub schema_version: u32,
    pub engine: Engine,
    pub ai_lines: BTreeMap<String, AiLineMeta>,
    pub deleted: Vec<String>,
    /// Raw-note anchors this document was generated from (needed to tell the
    /// user's own lines from lines typed into the enhanced document).
    pub anchors: Vec<AnchorRef>,
    /// Hash of the raw notes text and of the transcript used.
    pub raw_hash: String,
    pub transcript_hash: String,
}

impl Metadata {
    /// Parse stored `generation_metadata_json`; `None` for empty/legacy values.
    pub fn from_json_str(s: &str) -> Option<Self> {
        let meta: Metadata = serde_json::from_str(s).ok()?;
        (meta.schema_version > 0 && meta.engine.name == "enhanced-notes").then_some(meta)
    }
}

pub fn line_hash(text: &str) -> String {
    sha_hex(&normalize(text), 16)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileStats {
    pub replaced: usize,
    pub kept_edited: usize,
    pub kept_user: usize,
    pub suppressed: usize,
    pub deleted_honored: usize,
    pub dropped_with_anchor: usize,
}

#[derive(Debug, Clone)]
pub struct Reconciled {
    pub layout: Layout,
    pub metadata: Metadata,
    pub stats: ReconcileStats,
}

#[derive(Debug, Clone)]
struct Fresh {
    text: String,
    src: Vec<SourceRef>,
}

pub fn action_text(item: &ActionItem) -> String {
    match (&item.owner, &item.due) {
        (None, None) => item.text.clone(),
        (Some(o), None) => format!("{} (owner: {o})", item.text),
        (None, Some(d)) => format!("{} (due: {d})", item.text),
        (Some(o), Some(d)) => format!("{} (owner: {o}; due: {d})", item.text),
    }
}

fn fresh_groups(
    anchors: &[Anchor],
    output: &EnhanceOutput,
    segments: &[PromptSegment],
) -> BTreeMap<String, Vec<Fresh>> {
    let mut groups: BTreeMap<String, Vec<Fresh>> = BTreeMap::new();
    for section in &output.sections {
        if !anchors.iter().any(|a| a.key == section.anchor) {
            continue;
        }
        for b in &section.bullets {
            groups
                .entry(section.anchor.clone())
                .or_default()
                .push(Fresh {
                    text: b.text.clone(),
                    src: compact_sources(&b.sources, segments),
                });
        }
    }
    for b in &output.decisions {
        groups
            .entry(GROUP_DECISIONS.into())
            .or_default()
            .push(Fresh {
                text: b.text.clone(),
                src: compact_sources(&b.sources, segments),
            });
    }
    for item in &output.action_items {
        groups.entry(GROUP_ACTIONS.into()).or_default().push(Fresh {
            text: action_text(item),
            src: compact_sources(&item.sources, segments),
        });
    }
    for b in &output.open_questions {
        groups
            .entry(GROUP_QUESTIONS.into())
            .or_default()
            .push(Fresh {
                text: b.text.clone(),
                src: compact_sources(&b.sources, segments),
            });
    }
    groups
}

// ---------------------------------------------------------------------------
// Parsing the previously saved (possibly user-edited) enhanced document
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PrevLine {
    ai_id: Option<String>,
    text: String,
    node: Value,
    src: Vec<SourceRef>,
}

#[derive(Default)]
struct PrevState {
    groups: Vec<(String, Vec<PrevLine>)>,
    present_ids: HashSet<String>,
}

const TOP_GROUP: &str = "@top";

fn item_ai_id(node: &Value) -> Option<String> {
    node.get("attrs")?.get("aiId")?.as_str().map(str::to_owned)
}

fn is_ai_list(node: &Value) -> bool {
    node_type(node) == "bulletList" && children(node).iter().any(|i| item_ai_id(i).is_some())
}

struct Parser<'a> {
    expected: &'a [AnchorRef],
    next: usize,
    current: String,
    state: PrevState,
}

impl Parser<'_> {
    fn in_section(&self) -> bool {
        matches!(
            self.current.as_str(),
            GROUP_DECISIONS | GROUP_ACTIONS | GROUP_QUESTIONS
        )
    }

    fn push(&mut self, line: PrevLine) {
        if let Some(id) = &line.ai_id {
            self.state.present_ids.insert(id.clone());
        }
        let group = self.current.clone();
        match self.state.groups.iter_mut().find(|(g, _)| *g == group) {
            Some((_, lines)) => lines.push(line),
            None => self.state.groups.push((group, vec![line])),
        }
    }

    fn make_line(node: &Value) -> PrevLine {
        PrevLine {
            ai_id: item_ai_id(node),
            text: line_text(node),
            node: node.clone(),
            src: node
                .get("attrs")
                .and_then(|a| a.get("src"))
                .map(parse_src)
                .unwrap_or_default(),
        }
    }

    /// Does `text` equal an expected raw anchor at or after the cursor?
    fn match_expected(&mut self, text: &str) -> bool {
        let norm = normalize(text);
        for i in self.next..self.expected.len() {
            if normalize(&self.expected[i].text) == norm {
                self.current = self.expected[i].key.clone();
                self.next = i + 1;
                return true;
            }
        }
        false
    }

    fn peek_matches(&self, text: &str) -> bool {
        let norm = normalize(text);
        self.expected[self.next..]
            .iter()
            .any(|a| normalize(&a.text) == norm)
    }

    fn items_as_lines(&mut self, list: &Value) {
        for item in children(list) {
            if is_list_item(item) {
                self.push(Self::make_line(item));
            }
        }
    }

    fn walk_blocks(&mut self, nodes: &[Value], top_level: bool) {
        for node in nodes {
            match node_type(node) {
                "heading" | "paragraph" => {
                    let text = line_text(node);
                    if text.is_empty() {
                        continue;
                    }
                    if top_level && node_type(node) == "heading" {
                        if let Some(group) = section_group(&text)
                            && !self.peek_matches(&text)
                        {
                            self.current = group.into();
                            continue;
                        }
                        if normalize(&text) == normalize(HEADING_OVERVIEW)
                            && let Some(i) = self.expected[self.next..]
                                .iter()
                                .position(|a| a.key == OVERVIEW_KEY)
                        {
                            self.next += i + 1;
                            self.current = OVERVIEW_KEY.into();
                            continue;
                        }
                    }
                    if self.in_section() || !self.match_expected(&text) {
                        if self.current.is_empty() {
                            self.current = TOP_GROUP.into();
                        }
                        self.push(PrevLine {
                            ai_id: None,
                            text,
                            node: node.clone(),
                            src: vec![],
                        });
                    }
                }
                "bulletList" | "orderedList" | "taskList" => {
                    if self.in_section() || is_ai_list(node) {
                        self.items_as_lines(node);
                    } else {
                        for item in children(node) {
                            self.walk_raw_item(item);
                        }
                    }
                }
                "blockquote" => self.walk_blocks(children(node), false),
                _ => {}
            }
        }
    }

    fn walk_raw_item(&mut self, item: &Value) {
        if !is_list_item(item) {
            return;
        }
        let text = line_text(item);
        if !text.is_empty() && !self.match_expected(&text) {
            if self.current.is_empty() {
                self.current = TOP_GROUP.into();
            }
            // A line typed into the enhanced document: keep it whole.
            let mut line = Self::make_line(item);
            if item_ai_id(item).is_some() {
                // an AI item outside an AI list (copy/paste): treat as plain user text
                line.ai_id = None;
            }
            self.push(line);
        }
        for kid in children(item).iter().skip(1) {
            if is_ai_list(kid) {
                self.items_as_lines(kid);
            } else {
                self.walk_blocks(std::slice::from_ref(kid), false);
            }
        }
    }
}

fn parse_prev(doc: &Value, meta: &Metadata) -> PrevState {
    let mut parser = Parser {
        expected: &meta.anchors,
        next: 0,
        current: String::new(),
        state: PrevState::default(),
    };
    parser.walk_blocks(children(doc), true);
    parser.state
}

// ---------------------------------------------------------------------------
// Reconcile
// ---------------------------------------------------------------------------

enum Slot {
    Replaceable,
    Kept(Line),
}

fn new_id(group: &str, text: &str, used: &mut HashSet<String>) -> String {
    let base = format!(
        "ai_{}",
        sha_hex(&format!("{group}|{}", normalize(text)), 10)
    );
    let mut id = base.clone();
    let mut n = 2;
    while used.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    used.insert(id.clone());
    id
}

pub fn reconcile(
    prev_doc: Option<&Value>,
    prev_meta: Option<&Metadata>,
    anchors: &[Anchor],
    output: &EnhanceOutput,
    segments: &[PromptSegment],
    regenerate: bool,
    engine: Engine,
) -> Reconciled {
    let fresh = fresh_groups(anchors, output, segments);
    let anchor_keys: Vec<String> = anchors.iter().map(|a| a.key.clone()).collect();
    let mut group_order: Vec<String> = anchor_keys.clone();
    group_order.extend([GROUP_DECISIONS, GROUP_ACTIONS, GROUP_QUESTIONS].map(String::from));

    let mut stats = ReconcileStats::default();
    let empty_meta = Metadata::default();
    let (prev_doc, prev_meta) = match (prev_doc, prev_meta) {
        (Some(d), Some(m)) => (Some(d), m),
        _ => (None, &empty_meta),
    };
    let state = prev_doc
        .map(|d| parse_prev(d, prev_meta))
        .unwrap_or_default();

    // prev anchor key -> new group key (sections map to themselves).
    let mapping = rematch(&prev_meta.anchors, anchors);
    let map_group = |g: &str| -> Option<String> {
        if matches!(g, GROUP_DECISIONS | GROUP_ACTIONS | GROUP_QUESTIONS) {
            return Some(g.to_owned());
        }
        mapping.get(g).cloned()
    };
    let first_group = anchor_keys.first().cloned().unwrap_or_default();
    let fallback_group = |g: &str| -> Option<String> {
        if g == TOP_GROUP {
            return (!first_group.is_empty()).then(|| first_group.clone());
        }
        let idx = prev_meta.anchors.iter().position(|a| a.key == g);
        if let Some(idx) = idx {
            for earlier in prev_meta.anchors[..idx].iter().rev() {
                if let Some(t) = mapping.get(&earlier.key) {
                    return Some(t.clone());
                }
            }
        }
        (!first_group.is_empty()).then(|| first_group.clone())
    };

    // Newly deleted AI lines: tracked in metadata but gone from the document.
    let mut deleted_ids: Vec<String> = Vec::new();
    let mut deleted_entries: BTreeMap<String, AiLineMeta> = BTreeMap::new();
    if prev_doc.is_some() && !regenerate {
        let mut all: Vec<String> = prev_meta.deleted.clone();
        for id in prev_meta.ai_lines.keys() {
            if !prev_meta.deleted.contains(id) && !state.present_ids.contains(id) {
                all.push(id.clone());
            }
        }
        for id in all {
            let Some(entry) = prev_meta.ai_lines.get(&id) else {
                continue;
            };
            let Some(group) = map_group(&entry.anchor) else {
                continue; // its anchor is gone: nothing left to suppress
            };
            if deleted_ids.contains(&id) {
                continue;
            }
            deleted_ids.push(id.clone());
            deleted_entries.insert(
                id,
                AiLineMeta {
                    anchor: group,
                    ..entry.clone()
                },
            );
        }
        stats.deleted_honored = deleted_ids.len();
    }

    // Distribute previous lines to their new groups.
    let mut prev_by_group: HashMap<String, Vec<PrevLine>> = HashMap::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    for (group, lines) in state.groups {
        let target = map_group(&group);
        for line in lines {
            let known = line
                .ai_id
                .as_ref()
                .filter(|id| {
                    prev_meta.ai_lines.contains_key(*id)
                        && !prev_meta.deleted.contains(*id)
                        && seen_ids.insert((*id).clone())
                })
                .is_some();
            let ai_id = if known { line.ai_id.clone() } else { None };
            let line = PrevLine { ai_id, ..line };
            let unchanged_ai = known
                && prev_meta
                    .ai_lines
                    .get(line.ai_id.as_deref().unwrap_or(""))
                    .is_some_and(|e| e.hash == line_hash(&line.text));
            match &target {
                Some(t) => prev_by_group.entry(t.clone()).or_default().push(line),
                None => {
                    if unchanged_ai || (regenerate && known) {
                        stats.dropped_with_anchor += 1;
                    } else if let Some(t) = fallback_group(&group) {
                        prev_by_group.entry(t).or_default().push(line);
                    }
                }
            }
        }
    }

    let mut layout = Layout::new();
    let mut ai_lines: BTreeMap<String, AiLineMeta> = BTreeMap::new();
    let mut used_ids: HashSet<String> = deleted_ids.iter().cloned().collect();

    for group in &group_order {
        let prev_lines = prev_by_group.remove(group).unwrap_or_default();
        let fresh_lines: Vec<Fresh> = fresh.get(group).cloned().unwrap_or_default();

        let mut slots: Vec<Slot> = Vec::new();
        let mut suppress: Vec<String> = Vec::new();
        for pl in prev_lines {
            match &pl.ai_id {
                Some(id) => {
                    let entry = &prev_meta.ai_lines[id];
                    if regenerate {
                        continue;
                    }
                    if entry.hash == line_hash(&pl.text) {
                        slots.push(Slot::Replaceable);
                        continue;
                    }
                    suppress.push(pl.text.clone());
                    suppress.push(entry.text.clone());
                    stats.kept_edited += 1;
                    used_ids.insert(id.clone());
                    ai_lines.insert(
                        id.clone(),
                        AiLineMeta {
                            anchor: group.clone(),
                            ..entry.clone()
                        },
                    );
                    slots.push(Slot::Kept(Line {
                        ai_id: Some(id.clone()),
                        text: pl.text,
                        src: pl.src,
                        node: Some(pl.node),
                    }));
                }
                None => {
                    suppress.push(pl.text.clone());
                    stats.kept_user += 1;
                    slots.push(Slot::Kept(Line {
                        ai_id: None,
                        text: pl.text,
                        src: vec![],
                        node: Some(pl.node),
                    }));
                }
            }
        }
        for entry in deleted_entries.values().filter(|e| &e.anchor == group) {
            suppress.push(entry.text.clone());
        }

        let mut fresh_iter = fresh_lines
            .into_iter()
            .filter(|f| {
                let dup = suppress.iter().any(|s| {
                    !s.is_empty() && (is_duplicate(&f.text, s) || similarity(&f.text, s) >= 0.8)
                });
                if dup {
                    stats.suppressed += 1;
                }
                !dup
            })
            .collect::<Vec<_>>()
            .into_iter();

        let mut lines: Vec<Line> = Vec::new();
        let mut materialize = |f: Fresh, lines: &mut Vec<Line>| {
            let id = new_id(group, &f.text, &mut used_ids);
            ai_lines.insert(
                id.clone(),
                AiLineMeta {
                    hash: line_hash(&f.text),
                    anchor: group.clone(),
                    text: f.text.clone(),
                },
            );
            stats.replaced += 1;
            lines.push(Line {
                ai_id: Some(id),
                text: f.text,
                src: f.src,
                node: None,
            });
        };
        for slot in slots {
            match slot {
                Slot::Kept(line) => lines.push(line),
                Slot::Replaceable => {
                    if let Some(f) = fresh_iter.next() {
                        materialize(f, &mut lines);
                    }
                }
            }
        }
        for f in fresh_iter {
            materialize(f, &mut lines);
        }
        if !lines.is_empty() {
            layout.insert(group.clone(), lines);
        }
    }

    // Carry delete tombstones forward (bounded).
    for (id, entry) in &deleted_entries {
        ai_lines.insert(id.clone(), entry.clone());
    }
    if deleted_ids.len() > MAX_DELETED {
        let excess = deleted_ids.len() - MAX_DELETED;
        for id in deleted_ids.drain(..excess) {
            ai_lines.remove(&id);
        }
    }

    let metadata = Metadata {
        schema_version: METADATA_SCHEMA_VERSION,
        engine,
        ai_lines,
        deleted: deleted_ids,
        anchors: anchors
            .iter()
            .map(|a| AnchorRef {
                key: a.key.clone(),
                text: a.text.clone(),
            })
            .collect(),
        raw_hash: String::new(),
        transcript_hash: String::new(),
    };
    Reconciled {
        layout,
        metadata,
        stats,
    }
}
