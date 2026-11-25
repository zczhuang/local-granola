//! Chunking for long meetings. Every chunk carries all anchors and a slice of
//! the transcript; merging is deterministic and needs no extra model call.

use std::ops::Range;

use crate::anchors::Anchor;
use crate::schema::{ActionItem, Bullet, EnhanceOutput, Section, is_duplicate, union_sources};
use crate::segments::PromptSegment;
use crate::util::is_cjk;

/// CJK-aware token estimate: roughly one token per CJK character, one per ~3.6 other characters.
pub fn estimate_tokens(text: &str) -> usize {
    let (mut cjk, mut other) = (0usize, 0usize);
    for c in text.chars() {
        if is_cjk(c) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    cjk + (other * 10).div_ceil(36)
}

/// Share of `num_ctx` the prompt may occupy; the rest is left for the answer.
pub const PROMPT_BUDGET_RATIO: f64 = 0.6;

fn segment_tokens(segment: &PromptSegment) -> usize {
    estimate_tokens(&segment.prompt_line()) + 2
}

/// Split segments into index ranges. `fixed_tokens` is everything in the
/// prompt that is not transcript (system prompt, anchors, participants).
/// Returns a single full range when everything fits.
#[allow(clippy::single_range_in_vec_init)]
pub fn plan_chunks(
    segments: &[PromptSegment],
    fixed_tokens: usize,
    num_ctx: usize,
) -> Vec<Range<usize>> {
    if segments.is_empty() {
        return vec![0..0];
    }
    let limit = (num_ctx as f64 * PROMPT_BUDGET_RATIO) as usize;
    let budget = limit.saturating_sub(fixed_tokens).max(512);
    let costs: Vec<usize> = segments.iter().map(segment_tokens).collect();
    if costs.iter().sum::<usize>() <= budget {
        return vec![0..segments.len()];
    }
    let overlap_budget = budget / 10;
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < segments.len() {
        let mut end = start;
        let mut used = 0;
        while end < segments.len() && (end == start || used + costs[end] <= budget) {
            used += costs[end];
            end += 1;
        }
        ranges.push(start..end);
        if end >= segments.len() {
            break;
        }
        // Next chunk starts a little earlier so facts spanning the boundary are seen whole.
        let mut next = end;
        let mut overlap = 0;
        while next > start + 1 && overlap + costs[next - 1] <= overlap_budget {
            next -= 1;
            overlap += costs[next];
        }
        start = next.max(start + 1);
    }
    ranges
}

fn anchor_index(anchors: &[Anchor], key: &str) -> usize {
    anchors
        .iter()
        .position(|a| a.key == key)
        .unwrap_or(usize::MAX)
}

fn merge_bullets(into: &mut Vec<Bullet>, from: Vec<Bullet>) {
    for b in from {
        if let Some(existing) = into.iter_mut().find(|e| is_duplicate(&e.text, &b.text)) {
            union_sources(&mut existing.sources, &b.sources);
        } else {
            into.push(b);
        }
    }
}

/// Merge per-chunk outputs (already validated) deterministically: chunk order,
/// then in-chunk order; duplicates (normalized or near-identical text) are
/// collapsed with their sources unioned. Sections follow anchor order.
pub fn merge_outputs(outputs: Vec<EnhanceOutput>, anchors: &[Anchor]) -> EnhanceOutput {
    let mut merged = EnhanceOutput::default();
    for out in outputs {
        for section in out.sections {
            match merged
                .sections
                .iter_mut()
                .find(|s| s.anchor == section.anchor)
            {
                Some(existing) => merge_bullets(&mut existing.bullets, section.bullets),
                None => merged.sections.push(section),
            }
        }
        merge_bullets(&mut merged.decisions, out.decisions);
        merge_bullets(&mut merged.open_questions, out.open_questions);
        for item in out.action_items {
            if let Some(existing) = merged
                .action_items
                .iter_mut()
                .find(|e| is_duplicate(&e.text, &item.text))
            {
                union_sources(&mut existing.sources, &item.sources);
                if existing.owner.is_none() {
                    existing.owner = item.owner;
                }
                if existing.due.is_none() {
                    existing.due = item.due;
                }
            } else {
                merged.action_items.push(ActionItem { ..item });
            }
        }
    }
    merged
        .sections
        .sort_by_key(|s: &Section| anchor_index(anchors, &s.anchor));
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segments::SourceRef;

    fn seg(n: usize, text: &str) -> PromptSegment {
        PromptSegment {
            sid: format!("s{n}"),
            speaker: "A".into(),
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

    #[test]
    fn estimate_counts_cjk_heavier_per_char() {
        assert!(estimate_tokens("我們決定下週發佈") >= 8);
        assert!(estimate_tokens("abcdefghij") <= 4);
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn short_transcripts_are_one_chunk_and_long_ones_overlap() {
        let few: Vec<_> = (1..=5).map(|i| seg(i, "short text")).collect();
        assert_eq!(plan_chunks(&few, 100, 8192), vec![0..5]);

        let many: Vec<_> = (1..=400).map(|i| seg(i, &"word ".repeat(40))).collect();
        let plan = plan_chunks(&many, 500, 8192);
        assert!(plan.len() > 1);
        assert_eq!(plan.first().unwrap().start, 0);
        assert_eq!(plan.last().unwrap().end, 400);
        for pair in plan.windows(2) {
            assert!(pair[1].start < pair[0].end, "chunks overlap");
            assert!(pair[1].start > pair[0].start, "chunks progress");
        }
        assert_eq!(plan, plan_chunks(&many, 500, 8192), "deterministic");
    }

    #[test]
    fn oversized_single_segment_still_makes_progress() {
        let big = vec![seg(1, &"x".repeat(100_000)), seg(2, "tail")];
        let plan = plan_chunks(&big, 0, 4096);
        assert_eq!(plan.last().unwrap().end, 2);
    }

    #[test]
    fn merge_dedupes_and_unions_sources_in_anchor_order() {
        let anchors = crate::anchors::extract_anchors(&serde_json::json!({"type":"doc","content":[
            {"type":"paragraph","content":[{"type":"text","text":"One"}]},
            {"type":"paragraph","content":[{"type":"text","text":"Two"}]}]}));
        let b = |t: &str, s: &[&str]| Bullet {
            text: t.into(),
            sources: s.iter().map(|x| x.to_string()).collect(),
        };
        let c1 = EnhanceOutput {
            sections: vec![Section {
                anchor: anchors[1].key.clone(),
                bullets: vec![b("Budget is 50k", &["s2"])],
            }],
            decisions: vec![b("Ship Friday", &["s3"])],
            action_items: vec![ActionItem {
                text: "Email Sam".into(),
                owner: None,
                due: None,
                sources: vec!["s3".into()],
            }],
            ..Default::default()
        };
        let c2 = EnhanceOutput {
            sections: vec![
                Section {
                    anchor: anchors[0].key.clone(),
                    bullets: vec![b("Intro done", &["s9"])],
                },
                Section {
                    anchor: anchors[1].key.clone(),
                    bullets: vec![b("budget is 50K", &["s8"])],
                },
            ],
            decisions: vec![b("Ship Friday.", &["s4"])],
            action_items: vec![ActionItem {
                text: "email sam".into(),
                owner: Some("Alex".into()),
                due: Some("Mon".into()),
                sources: vec!["s4".into()],
            }],
            ..Default::default()
        };
        let merged = merge_outputs(vec![c1.clone(), c2.clone()], &anchors);
        assert_eq!(merged.sections[0].anchor, anchors[0].key);
        assert_eq!(merged.sections[1].bullets.len(), 1);
        assert_eq!(merged.sections[1].bullets[0].sources, vec!["s2", "s8"]);
        assert_eq!(merged.decisions.len(), 1);
        assert_eq!(merged.decisions[0].sources, vec!["s3", "s4"]);
        assert_eq!(merged.action_items[0].owner.as_deref(), Some("Alex"));
        assert_eq!(merged.action_items[0].due.as_deref(), Some("Mon"));
        assert_eq!(merged, merge_outputs(vec![c1, c2], &anchors));
    }
}
