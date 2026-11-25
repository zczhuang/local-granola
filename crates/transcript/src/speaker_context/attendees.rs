//! Maps the diarizer's remote speaker clusters to the meeting's attendees.
//!
//! Evidence is used in a fixed order, each stage only filling clusters and attendees the stages
//! before it left open:
//!
//! 1. Names the meeting app showed while a cluster spoke (Accessibility or the Meet extension),
//!    fuzzily matched to attendee names and email addresses.
//! 2. Voiceprint scores, only where one pairing is clearly the best for both sides.
//! 3. Elimination, when exactly one cluster and one attendee are left and the counts match.
//!
//! One attendee per cluster and one cluster per attendee. The local user (the microphone's human)
//! is never assigned to a remote cluster: their voice is the microphone, not a cluster.
//! Nothing here guesses: a cluster with weak or conflicting evidence stays unassigned.

use std::collections::{BTreeMap, HashMap, HashSet};

use anlg_voiceprint::{
    MIN_UNIQUE_MARGIN, MIN_UNIQUE_SCORE, VoiceprintSpeakerKey, complete_by_elimination,
    pick_unique_voiceprint_assignments,
};

use super::MeetingNames;
use crate::meeting_speakers::{MeetingSpeakerEvidence, VoiceWord};

/// A remote speaker cluster: the diarizer's channel and speaker index.
pub type ClusterKey = VoiceprintSpeakerKey;

/// A cluster is named by app cues when one attendee holds this share of its voted speech...
const NAME_SHARE: f64 = 0.6;
/// ...with at least this much voted speech.
const MIN_VOTED_MS: i64 = 2_000;
/// The least similar a shown name may be to an attendee's name or email and still count.
const MIN_NAME_SIMILARITY: f64 = 0.75;
/// A second attendee this close to the best match makes the name ambiguous.
const NAME_MARGIN: f64 = 0.1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attendee {
    /// Stable id the caller maps results back with (a human id, or the calendar email).
    pub id: String,
    pub name: Option<String>,
    pub emails: Vec<String>,
    /// The microphone's human. Never assigned to a cluster.
    pub is_me: bool,
}

/// A name the meeting app showed while `cluster` was speaking, for `ms` of that speech.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterVote {
    pub cluster: ClusterKey,
    pub name: String,
    pub ms: i64,
}

/// How well `cluster`'s voice matched `attendee_id`'s stored voiceprint, as a cosine similarity.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceprintScore {
    pub cluster: ClusterKey,
    pub attendee_id: String,
    pub score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentSource {
    /// A name the meeting app showed.
    AppName,
    /// A stored voiceprint.
    Voiceprint,
    /// The only cluster and attendee left over.
    Elimination,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterAssignment {
    pub cluster: ClusterKey,
    pub attendee_id: String,
    pub source: AssignmentSource,
}

/// One transcribed word of a remote cluster, in milliseconds from its transcript's start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterWord {
    pub cluster: ClusterKey,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// What the meeting app's evidence (Accessibility cues, the Meet extension) says about each remote
/// cluster: the name the app showed while the cluster spoke, per cluster, with the speech it
/// covers. The evidence decides which of a cluster's voices is clearly named (the same rules the
/// transcript render uses); this only re-keys that verdict by cluster so
/// [`map_clusters_to_attendees`] can match the names to attendees.
///
/// `started_at` is the epoch-ms start of the transcript the words belong to and
/// `transcript_starts` those of every transcript in the session (a later one bounds a capture
/// whose evidence never closed).
pub fn meeting_cluster_votes(
    meetings: &[MeetingSpeakerEvidence],
    transcript_starts: &[i64],
    started_at: i64,
    words: &[ClusterWord],
) -> Vec<ClusterVote> {
    if meetings.is_empty() || words.is_empty() {
        return Vec::new();
    }
    let probe = MeetingNames {
        meetings,
        transcript_starts,
        started_at,
        names: Vec::new(),
    };
    let mut by_capture: Vec<Vec<(VoiceWord, ClusterKey)>> =
        meetings.iter().map(|_| Vec::new()).collect();
    for word in words {
        let Some((index, capture_ms)) = probe.capture_at(started_at.saturating_add(word.start_ms))
        else {
            continue;
        };
        let voice = meetings[index].voice_at(
            capture_ms,
            word.cluster
                .speaker_index
                .and_then(|index| i32::try_from(index).ok()),
        );
        by_capture[index].push((
            VoiceWord {
                voice,
                start_ms: capture_ms,
                end_ms: capture_ms + (word.end_ms - word.start_ms),
            },
            word.cluster,
        ));
    }

    let mut tally: BTreeMap<((i64, Option<i64>), String), (ClusterKey, i64)> = BTreeMap::new();
    for (evidence, words) in meetings.iter().zip(&by_capture) {
        let voice_words: Vec<VoiceWord> = words.iter().map(|(word, _)| word.clone()).collect();
        let named = evidence.name_voices(&voice_words);
        for (word, cluster) in words {
            let Some(voice) = named.get(&word.voice) else {
                continue;
            };
            let entry = tally
                .entry((cluster_order(cluster), voice.name.clone()))
                .or_insert((*cluster, 0));
            entry.1 += (word.end_ms - word.start_ms).max(0);
        }
    }
    tally
        .into_iter()
        .map(|((_, name), (cluster, ms))| ClusterVote { cluster, name, ms })
        .collect()
}

/// Assigns clusters to attendees. The clusters are every one seen in `cluster_votes` and
/// `voiceprint_scores`; the result is ordered by cluster.
pub fn map_clusters_to_attendees(
    cluster_votes: &[ClusterVote],
    voiceprint_scores: &[VoiceprintScore],
    attendees: &[Attendee],
) -> Vec<ClusterAssignment> {
    let remote: Vec<&Attendee> = attendees.iter().filter(|a| !a.is_me).collect();
    let remote_ids: HashSet<&str> = remote.iter().map(|a| a.id.as_str()).collect();

    let mut clusters: Vec<ClusterKey> = cluster_votes
        .iter()
        .map(|vote| vote.cluster)
        .chain(voiceprint_scores.iter().map(|score| score.cluster))
        .collect();
    clusters.sort_by_key(cluster_order);
    clusters.dedup();

    let mut assigned: BTreeMap<(i64, Option<i64>), ClusterAssignment> = BTreeMap::new();
    let taken_cluster = |assigned: &BTreeMap<_, ClusterAssignment>, cluster: &ClusterKey| {
        assigned.contains_key(&cluster_order(cluster))
    };

    // 1. Names the app showed.
    let mut named: Vec<(ClusterKey, usize, i64)> = Vec::new();
    for cluster in &clusters {
        if let Some((attendee, ms)) = name_leader(cluster, cluster_votes, &remote) {
            named.push((*cluster, attendee, ms));
        }
    }
    // An attendee two clusters both claim goes to the one with more voted speech.
    named.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then(cluster_order(&a.0).cmp(&cluster_order(&b.0)))
    });
    let mut used_attendees: HashSet<usize> = HashSet::new();
    for (cluster, attendee, _) in named {
        if used_attendees.insert(attendee) {
            assigned.insert(
                cluster_order(&cluster),
                ClusterAssignment {
                    cluster,
                    attendee_id: remote[attendee].id.clone(),
                    source: AssignmentSource::AppName,
                },
            );
        }
    }

    // 2. Voiceprints, among what is left.
    let open_scores: Vec<(ClusterKey, &str, f32)> = voiceprint_scores
        .iter()
        .filter(|score| remote_ids.contains(score.attendee_id.as_str()))
        .filter(|score| !taken_cluster(&assigned, &score.cluster))
        .filter(|score| {
            !assigned
                .values()
                .any(|existing| existing.attendee_id == score.attendee_id)
        })
        .map(|score| (score.cluster, score.attendee_id.as_str(), score.score))
        .collect();
    for pick in
        pick_unique_voiceprint_assignments(&open_scores, MIN_UNIQUE_SCORE, MIN_UNIQUE_MARGIN)
    {
        assigned.insert(
            cluster_order(&pick.speaker),
            ClusterAssignment {
                cluster: pick.speaker,
                attendee_id: pick.human_id,
                source: AssignmentSource::Voiceprint,
            },
        );
    }

    // 3. Elimination: as many clusters as remote attendees, all but one paired.
    let people: Vec<String> = remote.iter().map(|a| a.id.clone()).collect();
    let assigned_clusters: Vec<ClusterKey> = assigned.values().map(|a| a.cluster).collect();
    let assigned_people: Vec<&str> = assigned.values().map(|a| a.attendee_id.as_str()).collect();
    if let Some((cluster, attendee_id)) =
        complete_by_elimination(&clusters, &people, &assigned_clusters, &assigned_people)
    {
        assigned.insert(
            cluster_order(&cluster),
            ClusterAssignment {
                cluster,
                attendee_id,
                source: AssignmentSource::Elimination,
            },
        );
    }

    assigned.into_values().collect()
}

fn cluster_order(cluster: &ClusterKey) -> (i64, Option<i64>) {
    (cluster.channel, cluster.speaker_index)
}

/// The one remote attendee holding a clear majority of `cluster`'s voted speech.
fn name_leader(
    cluster: &ClusterKey,
    votes: &[ClusterVote],
    remote: &[&Attendee],
) -> Option<(usize, i64)> {
    let mut by_attendee: HashMap<usize, i64> = HashMap::new();
    let mut voted_ms = 0;
    for vote in votes
        .iter()
        .filter(|vote| vote.cluster == *cluster && vote.ms > 0)
    {
        voted_ms += vote.ms;
        if let Some(index) = match_attendee(&vote.name, remote) {
            *by_attendee.entry(index).or_default() += vote.ms;
        }
    }
    let (&attendee, &ms) = by_attendee
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))?;
    (ms >= MIN_VOTED_MS && ms as f64 >= NAME_SHARE * voted_ms as f64).then_some((attendee, ms))
}

/// The attendee a shown name most plausibly belongs to, when one clearly does.
pub(crate) fn match_attendee(shown: &str, attendees: &[&Attendee]) -> Option<usize> {
    let shown = normalize(shown);
    if shown.is_empty() {
        return None;
    }
    let mut scored: Vec<(usize, f64)> = attendees
        .iter()
        .enumerate()
        .map(|(index, attendee)| (index, attendee_similarity(&shown, attendee)))
        .filter(|(_, score)| *score >= MIN_NAME_SIMILARITY)
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    match scored.as_slice() {
        [] => None,
        [(index, _)] => Some(*index),
        [(index, best), (_, second), ..] => (best - second >= NAME_MARGIN).then_some(*index),
    }
}

fn attendee_similarity(shown: &str, attendee: &Attendee) -> f64 {
    let names = attendee.name.iter().map(|name| normalize(name));
    let emails = attendee.emails.iter().flat_map(|email| {
        let local = email.split('@').next().unwrap_or_default();
        [normalize(email), normalize(local)]
    });
    names
        .chain(emails)
        .filter(|candidate| !candidate.is_empty())
        .map(|candidate| similarity(shown, &candidate))
        .fold(0.0, f64::max)
}

fn similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let (mut left, mut right): (Vec<&str>, Vec<&str>) =
        (a.split(' ').collect(), b.split(' ').collect());
    left.sort_unstable();
    right.sort_unstable();
    if left == right {
        return 0.95;
    }
    // "John" for "John Doe", or the reverse: every token of the shorter name is in the longer.
    let (short, long) = if left.len() <= right.len() {
        (&left, &right)
    } else {
        (&right, &left)
    };
    let subset = short.len() < long.len()
        && short
            .iter()
            .all(|token| token.chars().count() >= 2 && long.contains(token));
    let edit = {
        let (x, y): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        let longest = x.len().max(y.len()) as f64;
        1.0 - levenshtein(&x, &y) as f64 / longest
    };
    let edit = if edit >= 0.85 { edit } else { 0.0 };
    f64::max(if subset { MIN_NAME_SIMILARITY } else { 0.0 }, edit)
}

/// Lowercase words only: parenthesized roles ("(Host)", "(Guest)") and punctuation are dropped, and
/// `.`, `_`, `-` split words so an email's local part reads like a name.
fn normalize(text: &str) -> String {
    let mut depth = 0u32;
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            _ if ch.is_alphanumeric() => out.extend(ch.to_lowercase()),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(ca != cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(index: i64) -> ClusterKey {
        ClusterKey {
            channel: 1,
            speaker_index: Some(index),
        }
    }

    fn person(id: &str, name: &str, email: &str, is_me: bool) -> Attendee {
        Attendee {
            id: id.into(),
            name: (!name.is_empty()).then(|| name.into()),
            emails: if email.is_empty() {
                vec![]
            } else {
                vec![email.into()]
            },
            is_me,
        }
    }

    fn vote(index: i64, name: &str, ms: i64) -> ClusterVote {
        ClusterVote {
            cluster: key(index),
            name: name.into(),
            ms,
        }
    }

    fn score(index: i64, id: &str, score: f32) -> VoiceprintScore {
        VoiceprintScore {
            cluster: key(index),
            attendee_id: id.into(),
            score,
        }
    }

    fn table(assignments: &[ClusterAssignment]) -> Vec<(i64, &str, AssignmentSource)> {
        assignments
            .iter()
            .map(|a| {
                (
                    a.cluster.speaker_index.unwrap(),
                    a.attendee_id.as_str(),
                    a.source,
                )
            })
            .collect()
    }

    fn team() -> Vec<Attendee> {
        vec![
            person("me", "John Jeong", "john@example.com", true),
            person("alice", "Alice Kim", "alice.kim@example.com", false),
            person("bob", "Bob Lee", "blee@example.com", false),
            person("dana", "Dana Park", "dana@example.com", false),
        ]
    }

    #[test]
    fn app_names_assign_clusters_with_fuzzy_name_and_email_matching() {
        let votes = [
            vote(0, "Alice Kim (Guest)", 5_000),
            vote(1, "lee bob", 4_000),
            vote(2, "dana.park", 3_000),
        ];
        let result = map_clusters_to_attendees(&votes, &[], &team());
        assert_eq!(
            table(&result),
            vec![
                (0, "alice", AssignmentSource::AppName),
                (1, "bob", AssignmentSource::AppName),
                (2, "dana", AssignmentSource::AppName),
            ]
        );
    }

    #[test]
    fn a_minor_misspelling_and_a_first_name_alone_still_match_when_unambiguous() {
        let votes = [vote(0, "Alise Kim", 5_000), vote(1, "Bob", 5_000)];
        let result = map_clusters_to_attendees(&votes, &[], &team());
        assert_eq!(
            table(&result),
            vec![
                (0, "alice", AssignmentSource::AppName),
                (1, "bob", AssignmentSource::AppName),
            ]
        );
    }

    #[test]
    fn ambiguous_or_unknown_names_assign_nothing() {
        let mut attendees = team();
        attendees.push(person("alex", "Alice Kang", "", false));
        // "Alice" fits two attendees equally.
        let result = map_clusters_to_attendees(&[vote(0, "Alice", 9_000)], &[], &attendees);
        assert!(result.is_empty());
        let result = map_clusters_to_attendees(&[vote(0, "Zed Quinn", 9_000)], &[], &team());
        assert!(result.is_empty());
    }

    #[test]
    fn a_name_needs_enough_speech_and_a_clear_majority() {
        // Too little voted speech.
        assert!(map_clusters_to_attendees(&[vote(0, "Alice Kim", 1_500)], &[], &team()).is_empty());
        // A split vote.
        let split = [vote(0, "Alice Kim", 3_000), vote(0, "Bob Lee", 2_800)];
        assert!(map_clusters_to_attendees(&split, &[], &team()).is_empty());
        // A clear leader wins despite a backchannel.
        let clear = [vote(0, "Alice Kim", 8_000), vote(0, "Bob Lee", 1_000)];
        assert_eq!(
            table(&map_clusters_to_attendees(&clear, &[], &team())),
            vec![(0, "alice", AssignmentSource::AppName)]
        );
    }

    #[test]
    fn the_mic_human_is_never_given_a_remote_cluster() {
        // The app shows the local user's own name against a remote cluster.
        let votes = [vote(0, "John Jeong", 9_000)];
        assert!(map_clusters_to_attendees(&votes, &[], &team()).is_empty());
        // Nor does a voiceprint match, or elimination, hand them one.
        let scores = [score(0, "me", 0.99)];
        assert!(map_clusters_to_attendees(&[], &scores, &team()).is_empty());
        let attendees = [
            person("me", "John Jeong", "", true),
            person("alice", "Alice Kim", "", false),
        ];
        let result = map_clusters_to_attendees(&[vote(0, "Alice Kim", 9_000)], &[], &attendees);
        assert!(result.iter().all(|a| a.attendee_id != "me"));
    }

    #[test]
    fn one_attendee_per_cluster_and_the_louder_cluster_keeps_a_contested_name() {
        let votes = [vote(0, "Alice Kim", 4_000), vote(1, "Alice Kim", 9_000)];
        let result = map_clusters_to_attendees(&votes, &[], &team());
        assert_eq!(
            table(&result),
            vec![(1, "alice", AssignmentSource::AppName)]
        );
    }

    #[test]
    fn voiceprints_fill_clusters_the_app_never_named() {
        let votes = [vote(0, "Alice Kim", 5_000)];
        let scores = [
            score(1, "bob", 0.9),
            score(1, "dana", 0.5),
            score(2, "dana", 0.88),
            score(2, "bob", 0.4),
            // Alice already holds cluster 0; a high score for cluster 1 is ignored.
            score(1, "alice", 0.95),
        ];
        let result = map_clusters_to_attendees(&votes, &scores, &team());
        assert_eq!(
            table(&result),
            vec![
                (0, "alice", AssignmentSource::AppName),
                (1, "bob", AssignmentSource::Voiceprint),
                (2, "dana", AssignmentSource::Voiceprint),
            ]
        );
    }

    #[test]
    fn weak_or_close_voiceprints_assign_nothing() {
        let weak = [score(0, "alice", 0.4)];
        assert!(map_clusters_to_attendees(&[], &weak, &team()).is_empty());
        let close = [score(0, "alice", 0.9), score(0, "bob", 0.88)];
        assert!(map_clusters_to_attendees(&[], &close, &team()).is_empty());
    }

    #[test]
    fn elimination_names_the_last_cluster_only_when_the_counts_match() {
        let votes = [vote(0, "Alice Kim", 5_000), vote(1, "Bob Lee", 5_000)];
        // A third cluster, seen only through a weak score, and a third remote attendee.
        let scores = [score(2, "dana", 0.3)];
        let result = map_clusters_to_attendees(&votes, &scores, &team());
        assert_eq!(
            table(&result),
            vec![
                (0, "alice", AssignmentSource::AppName),
                (1, "bob", AssignmentSource::AppName),
                (2, "dana", AssignmentSource::Elimination),
            ]
        );
        // A no-show: two remote clusters can't be mapped onto three remote attendees.
        let result = map_clusters_to_attendees(&votes[..1], &[score(1, "dana", 0.3)], &team());
        assert_eq!(
            table(&result),
            vec![(0, "alice", AssignmentSource::AppName)]
        );
    }

    #[test]
    fn elimination_never_uses_the_mic_human_as_the_remaining_person() {
        // Two clusters and two attendees, one of whom is the mic human: only one remote person.
        let attendees = [
            person("me", "John Jeong", "", true),
            person("alice", "Alice Kim", "", false),
        ];
        let votes = [vote(0, "Alice Kim", 5_000)];
        let result = map_clusters_to_attendees(&votes, &[score(1, "me", 0.2)], &attendees);
        assert_eq!(
            table(&result),
            vec![(0, "alice", AssignmentSource::AppName)]
        );
    }

    #[test]
    fn normalize_drops_roles_punctuation_and_case() {
        assert_eq!(normalize("Alex (Host, me)"), "alex");
        assert_eq!(normalize("Dana.Park_x"), "dana park x");
        assert_eq!(normalize("  Åsa   Berg "), "åsa berg");
    }

    #[test]
    fn non_latin_names_match_exactly_or_by_close_edit() {
        let attendees = [
            person("jh", "지헌", "", false),
            person("dh", "덕행", "", false),
        ];
        let votes = [vote(0, "지헌", 5_000), vote(1, "덕행 (Guest)", 5_000)];
        let result = map_clusters_to_attendees(&votes, &[], &attendees);
        assert_eq!(
            table(&result),
            vec![
                (0, "jh", AssignmentSource::AppName),
                (1, "dh", AssignmentSource::AppName),
            ]
        );
    }
}
