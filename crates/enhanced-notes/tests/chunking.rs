mod common;

use common::*;
use enhanced_notes::schema::Section;
use enhanced_notes::{EnhanceError, EnhanceOutput, ExtractClient, TranscriptInput, Word, enhance};
use serde_json::Value;
use std::sync::Mutex;

/// Echoes which transcript slice it was shown, so chunking and merging are observable.
struct ChunkEcho {
    prompts: Mutex<Vec<String>>,
}

fn sids(user: &str) -> Vec<String> {
    user.lines()
        .filter(|l| l.starts_with('s') && l.contains(" ["))
        .filter_map(|l| l.split_whitespace().next().map(str::to_owned))
        .collect()
}

impl ExtractClient for ChunkEcho {
    fn model_name(&self) -> String {
        "echo".into()
    }
    async fn extract(
        &self,
        _s: &str,
        user: &str,
        _schema: &Value,
    ) -> Result<EnhanceOutput, EnhanceError> {
        let n = {
            let mut prompts = self.prompts.lock().unwrap();
            prompts.push(user.to_owned());
            prompts.len() - 1
        };
        const TOPICS: [&str; 12] = [
            "Budget approval for the data warehouse",
            "Customer onboarding friction in Germany",
            "Security audit scheduling with vendors",
            "Roadmap for the mobile redesign",
            "Support staffing over the holiday season",
            "Partnership terms with the logistics firm",
            "Migration of billing to the new ledger",
            "Quarterly hiring plan for infrastructure",
            "Legal review of the data processing addendum",
            "Pricing experiment results for annual plans",
            "Offsite agenda and travel logistics",
            "Retrospective on the incident last month",
        ];
        let ids = sids(user);
        let (first, last) = (ids.first().unwrap().clone(), ids.last().unwrap().clone());
        Ok(EnhanceOutput {
            sections: vec![Section {
                anchor: "a1".into(),
                bullets: vec![bullet(TOPICS[n % 12], &[&first])],
            }],
            decisions: vec![bullet("Ship the beta", &[&last])],
            ..Default::default()
        })
    }
}

fn long_transcript(sentences: usize) -> TranscriptInput {
    let mut words = Vec::new();
    let mut t = 0;
    for i in 0..sentences {
        for j in 0..14 {
            words.push(Word {
                id: format!("w{i}_{j}"),
                text: format!(" word{i}x{j}"),
                start_ms: t,
                end_ms: t + 200,
                channel: (i % 2) as i32,
                speaker_index: None,
            });
            t += 200;
        }
        t += 4000;
    }
    TranscriptInput {
        transcript_id: "long".into(),
        words,
    }
}

#[tokio::test]
async fn long_meetings_are_chunked_with_all_anchors_and_merged_deterministically() {
    let make = || {
        let mut i = input(raw_notes());
        i.transcripts = vec![long_transcript(500)];
        i.options.num_ctx = 8192;
        i
    };
    let c1 = ChunkEcho {
        prompts: Mutex::new(vec![]),
    };
    let r1 = enhance(make(), &c1).await.unwrap();
    assert!(r1.chunks > 1, "expected several chunks, got {}", r1.chunks);

    let prompts = c1.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), r1.chunks);
    for p in &prompts {
        // Every chunk carries every anchor.
        for needle in [
            "a1: Q3 launch plan",
            "a2: Pricing",
            "a3: Hiring",
            "a5: Risks",
        ] {
            assert!(p.contains(needle), "chunk lacks {needle}");
        }
    }
    // Chunks overlap by at least one segment.
    let first_chunk = sids(&prompts[0]);
    let second_chunk = sids(&prompts[1]);
    assert!(second_chunk.iter().any(|s| first_chunk.contains(s)));

    // Same input, same merged document (ids included).
    let c2 = ChunkEcho {
        prompts: Mutex::new(vec![]),
    };
    let r2 = enhance(make(), &c2).await.unwrap();
    assert_eq!(r1.doc, r2.doc);
    assert_eq!(r1.metadata, r2.metadata);

    // The repeated decision collapsed to one line carrying sources from every chunk.
    let ai = ai_items(&r1.doc);
    let decisions: Vec<_> = ai.iter().filter(|(_, t, _)| t == "Ship the beta").collect();
    assert_eq!(decisions.len(), 1);
    let src = enhanced_notes::render::parse_src(&decisions[0].2["attrs"]["src"]);
    assert!(
        src.len() >= r1.chunks.min(2),
        "sources unioned across chunks"
    );
    // One covering bullet per chunk under the first anchor.
    let topic_bullets = ai
        .iter()
        .filter(|(_, t, _)| t.as_str() != "Ship the beta" && t.len() > 20 && !t.contains("owner"))
        .count();
    assert!(topic_bullets >= r1.chunks);
}

#[tokio::test]
async fn cjk_transcripts_get_the_script_instruction() {
    let words = "我們決定下週發佈新版本"
        .chars()
        .enumerate()
        .map(|(i, c)| Word {
            id: format!("c{i}"),
            text: c.to_string(),
            start_ms: i as i64 * 200,
            end_ms: i as i64 * 200 + 200,
            channel: 0,
            speaker_index: None,
        })
        .collect();
    let mut i = input(raw_notes());
    i.transcripts = vec![TranscriptInput {
        transcript_id: "z".into(),
        words,
    }];
    let c = ChunkEcho {
        prompts: Mutex::new(vec![]),
    };
    enhance(i, &c).await.unwrap();
    let p = c.prompts.lock().unwrap()[0].clone();
    assert!(p.contains("Traditional Chinese (繁體中文)"));
    assert!(p.contains("我們決定下週發佈新版本"));
}
