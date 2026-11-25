//! Prompt construction. The system prompt states the contract; the user prompt
//! carries anchors, participants and the numbered transcript.

use crate::anchors::Anchor;
use crate::segments::PromptSegment;
use crate::util::is_cjk;

pub const SYSTEM_PROMPT: &str = "\
You are the note-enhancement engine of a private, local meeting-notes app. You receive (1) the user's own typed notes, split into numbered lines called ANCHORS (a1, a2, ...), and (2) a diarized transcript split into numbered SEGMENTS (s1, s2, ...). Your job is to add supporting detail from the transcript under each of the user's lines, plus lists of decisions, action items and open questions.

Hard rules:
1. Never rewrite, reorder, merge, translate or comment on the user's lines. You do not output them; you only reference them by anchor id.
2. Use only anchor ids that appear in the ANCHORS list and only segment ids that appear in the TRANSCRIPT. Never invent ids.
3. Every bullet, decision, action item and open question must be supported by at least one segment. List the supporting segment ids in \"sources\" (1 to 4 ids, the most direct ones).
4. State only facts, numbers, names, dates and commitments that are present in the cited segments. Do not guess, infer motives, add background knowledge or fill gaps. If the transcript does not cover an anchor, return no bullets for it.
5. Bullets are short, self-contained notes (one sentence, under 25 words), not copies of whole transcript passages. Do not repeat the user's line or another bullet. Prefer concrete detail (who, what, how much, by when) over generalities.
6. Decisions are things the participants clearly agreed or settled. Action items are concrete tasks someone committed to or was assigned; set \"owner\" only if a speaker label or a name in the segments makes it clear who, and \"due\" only if a date or deadline is stated (use the transcript's own wording, for example \"Friday\"); otherwise omit owner and due. Open questions are questions raised and left unresolved. Do not treat suggestions or ongoing discussion as decisions.
7. Empty arrays are valid and expected when there is nothing to report. Returning too little is better than including anything unsupported.
8. Language: write in the language the meeting is mostly spoken in (use the user's notes as a hint when it is mixed). For Chinese output follow the script instruction in the user message; the default is Traditional Chinese (繁體中文). Keep names, product names and numbers as spoken.
9. Output exactly one JSON object that matches the provided schema, and nothing else.";

/// Which Chinese script to write when the meeting is in Chinese.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChineseScript {
    #[default]
    Traditional,
    Simplified,
}

/// Part of a chunked transcript (1-based), if the transcript was split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkInfo {
    pub index: usize,
    pub total: usize,
}

pub fn is_mostly_cjk<'a>(texts: impl Iterator<Item = &'a str>) -> bool {
    let (mut cjk, mut letters) = (0usize, 0usize);
    for text in texts {
        for c in text.chars() {
            if is_cjk(c) {
                cjk += 1;
            } else if c.is_alphabetic() {
                letters += 1;
            }
        }
    }
    cjk > 0 && cjk * 2 >= cjk + letters
}

pub fn script_instruction(cjk: bool, script: ChineseScript) -> &'static str {
    match (cjk, script) {
        (false, _) => "Write in the language of the meeting.",
        (true, ChineseScript::Traditional) => {
            "The meeting is mostly Chinese. Write Traditional Chinese (繁體中文); never convert to Simplified."
        }
        (true, ChineseScript::Simplified) => {
            "The meeting is mostly Chinese. Write Simplified Chinese (简体中文)."
        }
    }
}

/// `anchors` are always all of them, even in a chunk; `segments` is the chunk.
pub fn build_user_prompt(
    anchors: &[Anchor],
    segments: &[PromptSegment],
    participants: &[String],
    script: ChineseScript,
    chunk: Option<ChunkInfo>,
) -> String {
    // The transcript decides the output language; typed notes only if there is no transcript text.
    let cjk = if segments.is_empty() {
        is_mostly_cjk(anchors.iter().map(|a| a.text.as_str()))
    } else {
        is_mostly_cjk(segments.iter().map(|s| s.text.as_str()))
    };
    let mut out = String::new();
    out.push_str("# Output language\n");
    out.push_str(script_instruction(cjk, script));
    out.push_str("\n\n# Participants\n");
    if participants.is_empty() {
        out.push_str("(not provided; speaker labels appear in the transcript)\n");
    } else {
        for p in participants {
            out.push_str(&format!("- {p}\n"));
        }
    }
    out.push_str("\n# ANCHORS (the user's own notes, in order; reference by id)\n");
    for a in anchors {
        let text = if a.synthetic {
            "(no typed notes: put an overview of the meeting here)"
        } else {
            a.text.as_str()
        };
        out.push_str(&format!("{}: {}\n", a.prompt_id(), text));
    }
    match chunk {
        Some(c) if c.total > 1 => {
            let (first, last) = (
                segments.first().map(|s| s.sid.as_str()).unwrap_or("-"),
                segments.last().map(|s| s.sid.as_str()).unwrap_or("-"),
            );
            out.push_str(&format!(
                "\n# TRANSCRIPT (part {} of {}; segments {first} to {last}; use only these segment ids; consecutive parts overlap slightly)\n",
                c.index, c.total
            ));
        }
        _ => out.push_str("\n# TRANSCRIPT\n"),
    }
    for s in segments {
        out.push_str(&s.prompt_line());
        out.push('\n');
    }
    out
}

pub const REPAIR_MESSAGE: &str = "Your previous reply was not valid JSON for the required schema. Reply again with only one JSON object with the keys sections, decisions, action_items and open_questions, and nothing else.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cjk_and_picks_script() {
        assert!(is_mostly_cjk(["我們決定下週發佈 ok"].into_iter()));
        assert!(!is_mostly_cjk(["we decided to ship"].into_iter()));
        assert!(script_instruction(true, ChineseScript::Traditional).contains("繁體"));
        assert!(script_instruction(true, ChineseScript::Simplified).contains("简体"));
        assert!(!script_instruction(false, ChineseScript::Traditional).contains("Chinese"));
    }

    #[test]
    fn system_prompt_states_the_contract() {
        for needle in [
            "Never rewrite",
            "Never invent ids",
            "sources",
            "Traditional Chinese",
            "Empty arrays",
        ] {
            assert!(
                SYSTEM_PROMPT.contains(needle) || SYSTEM_PROMPT.contains(&needle.to_lowercase()),
                "{needle}"
            );
        }
    }
}
