#![cfg_attr(target_os = "windows", allow(dead_code))]

use std::collections::HashSet;

use super::{AxNode, MeetingPlatform, node_labels, path_is_ancestor};

const MAX_EXTRACTED_PARTICIPANTS: usize = 50;

pub(super) fn extract_meeting_title(
    platform: &MeetingPlatform,
    window_title: Option<&str>,
    nodes: &[AxNode],
) -> Option<String> {
    if let Some(title) = window_title.and_then(|raw| clean_window_title(platform, raw)) {
        return Some(title);
    }

    if *platform == MeetingPlatform::Zoom {
        for node in nodes {
            if !matches!(node.role.as_deref(), Some("AXStaticText") | Some("AXText")) {
                continue;
            }
            for label in node_labels(node) {
                if let Some(topic) = extract_zoom_topic_label(label) {
                    return Some(topic);
                }
            }
        }
    }

    None
}

fn clean_window_title(platform: &MeetingPlatform, raw: &str) -> Option<String> {
    let mut title = strip_browser_suffix(raw.trim()).trim().to_string();
    if title.is_empty() {
        return None;
    }

    match platform {
        MeetingPlatform::Zoom => {
            for suffix in [
                " - Zoom Workplace",
                " | Zoom Workplace",
                " – Zoom Workplace",
                " - Zoom Meeting",
                " – Zoom Meeting",
                " - Zoom",
                " | Zoom",
                " – Zoom",
            ] {
                if let Some(stripped) = strip_suffix_ci(&title, suffix) {
                    title = stripped.trim().to_string();
                }
            }
            for prefix in [
                "Zoom Meeting - ",
                "Zoom Meeting – ",
                "Zoom Meeting: ",
                "Zoom Workplace - ",
                "Zoom - ",
            ] {
                if let Some(stripped) = strip_prefix_ci(&title, prefix) {
                    title = stripped.trim().to_string();
                }
            }
        }
        MeetingPlatform::GoogleMeet => {
            for prefix in ["Meet - ", "Meet – ", "Meet — ", "Google Meet - ", "Google Meet – "] {
                if let Some(stripped) = strip_prefix_ci(&title, prefix) {
                    title = stripped.trim().to_string();
                }
            }
            for suffix in [" - Google Meet", " – Google Meet", " | Google Meet"] {
                if let Some(stripped) = strip_suffix_ci(&title, suffix) {
                    title = stripped.trim().to_string();
                }
            }
            if is_google_meet_code(&title) {
                return None;
            }
        }
        MeetingPlatform::MicrosoftTeams => {
            for suffix in [
                " | Microsoft Teams",
                " - Microsoft Teams",
                " – Microsoft Teams",
                " | Teams",
            ] {
                if let Some(stripped) = strip_suffix_ci(&title, suffix) {
                    title = stripped.trim().to_string();
                }
            }
            for prefix in ["Meeting | ", "Call | "] {
                if let Some(stripped) = strip_prefix_ci(&title, prefix) {
                    title = stripped.trim().to_string();
                }
            }
        }
        MeetingPlatform::Webex => {
            for suffix in [" - Webex", " | Webex", " – Webex", " - Cisco Webex"] {
                if let Some(stripped) = strip_suffix_ci(&title, suffix) {
                    title = stripped.trim().to_string();
                }
            }
        }
        MeetingPlatform::Slack => {
            for suffix in [" - Slack", " | Slack"] {
                if let Some(stripped) = strip_suffix_ci(&title, suffix) {
                    title = stripped.trim().to_string();
                }
            }
            for prefix in ["Huddle in ", "Huddle - ", "Slack Huddle - "] {
                if let Some(stripped) = strip_prefix_ci(&title, prefix) {
                    title = stripped.trim().to_string();
                }
            }
        }
        MeetingPlatform::Discord => {
            for suffix in [" - Discord", " | Discord"] {
                if let Some(stripped) = strip_suffix_ci(&title, suffix) {
                    title = stripped.trim().to_string();
                }
            }
        }
        MeetingPlatform::Unknown => return None,
    }

    let trimmed = title.trim();
    if trimmed.is_empty() || is_generic_meeting_title(trimmed) {
        return None;
    }

    Some(trimmed.to_string())
}

fn extract_zoom_topic_label(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    for prefix in ["Meeting Topic:", "Topic:"] {
        if let Some(rest) = strip_prefix_ci(trimmed, prefix) {
            let topic = rest.trim();
            if !topic.is_empty() && !is_generic_meeting_title(topic) && topic.chars().count() <= 120
            {
                return Some(topic.to_string());
            }
        }
    }
    None
}

fn strip_browser_suffix(title: &str) -> &str {
    for suffix in [
        " - Google Chrome",
        " – Google Chrome",
        " - Chromium",
        " - Brave",
        " - Arc",
        " - Safari",
        " - Microsoft Edge",
        " - Mozilla Firefox",
        " - Vivaldi",
        " - Opera",
        " - Dia",
        " - Zen Browser",
        " - Comet",
    ] {
        if let Some(stripped) = strip_suffix_ci(title, suffix) {
            return stripped;
        }
    }
    title
}

fn is_google_meet_code(value: &str) -> bool {
    let parts: Vec<&str> = value.trim().split('-').collect();
    parts.len() == 3
        && parts[0].len() == 3
        && parts[1].len() == 4
        && parts[2].len() == 3
        && parts
            .iter()
            .all(|part| part.chars().all(|c| c.is_ascii_lowercase()))
}

fn is_generic_meeting_title(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "zoom"
            | "zoom meeting"
            | "zoom workplace"
            | "zoom webinar"
            | "meeting"
            | "leave meeting"
            | "end meeting"
            | "google meet"
            | "meet"
            | "microsoft teams"
            | "teams"
            | "teams meeting"
            | "webex"
            | "webex meeting"
            | "slack"
            | "huddle"
            | "slack huddle"
            | "discord"
            | "call"
            | "untitled"
    )
}

pub(super) fn extract_meeting_participants(
    platform: &MeetingPlatform,
    nodes: &[AxNode],
) -> Vec<String> {
    if *platform == MeetingPlatform::Unknown || nodes.is_empty() {
        return Vec::new();
    }

    let participant_list_paths: Vec<&[usize]> = nodes
        .iter()
        .filter(|node| is_participant_list_container(node))
        .map(|node| node.tree_path.as_slice())
        .collect();

    let mut seen = HashSet::<String>::new();
    let mut participants = Vec::new();

    let mut push_candidate = |raw_name: &str| {
        if participants.len() >= MAX_EXTRACTED_PARTICIPANTS {
            return;
        }
        let Some(cleaned) = clean_participant_name(raw_name) else {
            return;
        };
        let key = cleaned.to_ascii_lowercase();
        if seen.insert(key) {
            participants.push(cleaned);
        }
    };

    for node in nodes {
        if node.within_zoom_chat_scope {
            continue;
        }
        let in_participant_list = participant_list_paths
            .iter()
            .any(|path| path_is_ancestor(path, &node.tree_path));

        for label in node_labels(node) {
            if let Some(name) = extract_participant_from_tile_label(platform, label) {
                push_candidate(&name);
            } else if in_participant_list
                && matches!(
                    node.role.as_deref(),
                    Some("AXRow")
                        | Some("AXCell")
                        | Some("AXGroup")
                        | Some("AXStaticText")
                        | Some("AXText")
                        | Some("AXButton")
                )
                && let Some(name) = extract_participant_from_list_label(label)
            {
                push_candidate(&name);
            }
        }
    }

    participants
}

fn is_participant_list_container(node: &AxNode) -> bool {
    if !matches!(
        node.role.as_deref(),
        Some("AXList")
            | Some("AXTable")
            | Some("AXOutline")
            | Some("AXScrollArea")
            | Some("AXGroup")
            | Some("AXLandmark")
    ) {
        return false;
    }

    node_labels(node).any(|label| {
        let lower = label.trim().to_ascii_lowercase();
        lower == "participants"
            || lower == "participants list"
            || lower == "participant list"
            || lower == "people"
            || lower == "attendees"
            || lower == "in this meeting"
            || lower == "in the meeting"
            || lower.starts_with("participants (")
            || lower.starts_with("people (")
            || lower.starts_with("in this meeting (")
            || lower.contains("participant list")
    })
}

fn extract_participant_from_tile_label(platform: &MeetingPlatform, raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 160 {
        return None;
    }

    // Screen-share presenter labels across platforms
    if let Some(rest) = strip_prefix_ci(trimmed, "you are viewing ")
        && let Some(name) = strip_possessive_screen(rest)
    {
        return Some(name);
    }
    for suffix in [
        " is presenting",
        " is sharing their screen",
        " is sharing screen",
        " is sharing",
        "'s presentation",
        "’s presentation",
        "'s screen",
        "’s screen",
        " (presentation)",
    ] {
        if let Some(prefix) = strip_suffix_ci(trimmed, suffix) {
            return Some(prefix.trim().to_string());
        }
    }

    // Active-speaker / talking indicator labels (Zoom, Meet, Teams)
    for prefix in [
        "video render ",
        "talking: ",
        "speaking: ",
        "active speaker: ",
        "now speaking: ",
    ] {
        if let Some(rest) = strip_prefix_ci(trimmed, prefix) {
            return Some(strip_status_tail(rest));
        }
    }
    for suffix in [" is talking", " is speaking"] {
        if let Some(prefix) = strip_suffix_ci(trimmed, suffix) {
            return Some(strip_status_tail(prefix));
        }
    }

    // Comma-separated tile/status labels used heavily by Zoom, Teams, Meet, Webex:
    // e.g. "John Jeong, Computer audio muted", "Ada Lovelace (Host), Unmuted",
    //      "Stuart Chuang (Host, me), Computer audio unmuted"
    if let Some((head, tail)) = split_first_comma_outside_parens(trimmed) {
        let tail_lower = tail.trim().to_ascii_lowercase();
        let is_participant_status = tail_lower.contains("computer audio")
            || tail_lower.contains("audio muted")
            || tail_lower.contains("audio unmuted")
            || tail_lower.contains("microphone off")
            || tail_lower.contains("microphone on")
            || tail_lower.contains("mic muted")
            || tail_lower.contains("mic unmuted")
            || tail_lower.contains("video off")
            || tail_lower.contains("video on")
            || tail_lower.contains("camera off")
            || tail_lower.contains("camera on")
            || tail_lower.contains("hand raised")
            || tail_lower.contains("raised hand")
            || tail_lower.contains("spotlight")
            || tail_lower.contains("pinned")
            || tail_lower.starts_with("muted")
            || tail_lower.starts_with("unmuted")
            || tail_lower.starts_with("host")
            || tail_lower.starts_with("co-host")
            || tail_lower.starts_with("organizer")
            || tail_lower.starts_with("presenter")
            || tail_lower.starts_with("guest")
            || tail_lower.starts_with("external")
            || tail_lower.starts_with("talking")
            || tail_lower.starts_with("speaking");

        if is_participant_status {
            return Some(head.trim().to_string());
        }
    }

    // Platform-specific role tags directly on a tile without comma
    if matches!(
        platform,
        MeetingPlatform::Zoom
            | MeetingPlatform::GoogleMeet
            | MeetingPlatform::MicrosoftTeams
            | MeetingPlatform::Webex
    ) {
        let lower = trimmed.to_ascii_lowercase();
        if lower.ends_with(" (host)")
            || lower.ends_with(" (co-host)")
            || lower.ends_with(" (guest)")
            || lower.ends_with(" (external)")
            || lower.ends_with(" (organizer)")
            || lower.ends_with(" (presenter)")
            || lower.ends_with(" (me)")
            || lower.ends_with(" (you)")
        {
            return Some(trimmed.to_string());
        }
    }

    None
}

fn extract_participant_from_list_label(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 100 {
        return None;
    }
    Some(strip_status_tail(trimmed))
}

fn strip_possessive_screen(value: &str) -> Option<String> {
    let trimmed = value.trim();
    for suffix in ["'s screen", "’s screen", "'s shared screen", "’s shared screen"] {
        if let Some(prefix) = strip_suffix_ci(trimmed, suffix) {
            return Some(prefix.trim().to_string());
        }
    }
    None
}

fn split_first_comma_outside_parens(value: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (idx, ch) in value.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return Some((&value[..idx], &value[idx + 1..])),
            _ => {}
        }
    }
    None
}

fn strip_status_tail(raw: &str) -> String {
    split_first_comma_outside_parens(raw)
        .map(|(head, _)| head)
        .unwrap_or(raw)
        .trim()
        .to_string()
}

fn clean_participant_name(raw: &str) -> Option<String> {
    let mut name = raw.trim().to_string();
    if name.is_empty() {
        return None;
    }

    // If explicitly marked as the local user ("(Me)", "(You)", "(Host, me)"), skip so we don't
    // add the local user as a remote participant.
    let lower_initial = name.to_ascii_lowercase();
    if lower_initial == "you"
        || lower_initial == "me"
        || lower_initial == "yourself"
        || lower_initial.contains("(me)")
        || lower_initial.contains("(you)")
        || lower_initial.contains(", me)")
        || lower_initial.contains("(me,")
    {
        return None;
    }

    // Strip trailing Zoom/Meet/Teams role annotations in parentheses.
    loop {
        let before = name.clone();
        for suffix in [
            " (host)",
            " (co-host)",
            " (guest)",
            " (external)",
            " (organizer)",
            " (presenter)",
            " (attendee)",
            " (verified)",
            " (unverified)",
            " (in meeting)",
        ] {
            if let Some(stripped) = strip_suffix_ci(&name, suffix) {
                name = stripped.trim().to_string();
            }
        }
        if name == before {
            break;
        }
    }

    let name = name.trim_matches(|c: char| c == ',' || c == ':' || c == '-' || c.is_whitespace());
    if name.is_empty() || name.chars().count() < 2 || name.chars().count() > 64 {
        return None;
    }

    // Reject URLs, phone-only/digits-only strings, timestamps, or long sentences.
    if name.contains("http://")
        || name.contains("https://")
        || name.contains('\n')
        || name.contains('\r')
        || !name.chars().any(|c| c.is_alphabetic())
    {
        return None;
    }
    let word_count = name.split_whitespace().count();
    if word_count > 6 {
        return None;
    }

    if is_participant_chrome(name) {
        return None;
    }

    Some(name.to_string())
}

fn is_participant_chrome(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "mute"
            | "unmute"
            | "mute audio"
            | "unmute audio"
            | "mute my audio"
            | "unmute my audio"
            | "audio"
            | "computer audio"
            | "video"
            | "start video"
            | "stop video"
            | "camera"
            | "microphone"
            | "leave"
            | "leave meeting"
            | "leave call"
            | "end"
            | "end meeting"
            | "hang up"
            | "participants"
            | "participant"
            | "people"
            | "attendees"
            | "invite"
            | "invite people"
            | "add people"
            | "mute all"
            | "unmute all"
            | "ask all to unmute"
            | "chat"
            | "meeting chat"
            | "in-call messages"
            | "share"
            | "share screen"
            | "stop share"
            | "present now"
            | "record"
            | "recording"
            | "pause recording"
            | "stop recording"
            | "security"
            | "reactions"
            | "raise hand"
            | "lower hand"
            | "more"
            | "settings"
            | "audio settings"
            | "video settings"
            | "view"
            | "gallery"
            | "gallery view"
            | "speaker"
            | "speaker view"
            | "full screen"
            | "exit full screen"
            | "search"
            | "find a participant"
            | "search participants"
            | "close"
            | "cancel"
            | "done"
            | "ok"
            | "everyone"
            | "host"
            | "co-host"
            | "waiting room"
            | "in this meeting"
            | "in the meeting"
            | "in meeting"
            | "lobby"
            | "zoom"
            | "zoom meeting"
            | "zoom workplace"
            | "google meet"
            | "microsoft teams"
            | "webex"
            | "slack"
            | "huddle"
            | "no one else is here"
            | "you"
            | "me"
            | "yourself"
            | "your screen"
            | "unknown"
            | "caller"
    ) || lower.starts_with("participants (")
        || lower.starts_with("people (")
        || lower.starts_with("in this meeting (")
        || lower.starts_with("waiting room (")
        || lower.starts_with("muted ")
        || lower.starts_with("unmuted ")
        || lower.starts_with("turn on ")
        || lower.starts_with("turn off ")
        || lower.starts_with("open ")
        || lower.starts_with("close ")
        || lower.starts_with("show ")
        || lower.starts_with("hide ")
        || lower.starts_with("copy ")
        || lower.starts_with("pin ")
        || lower.starts_with("unpin ")
}

fn strip_prefix_ci<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let prefix_len = prefix.len();
    if value.len() < prefix_len || !value.is_char_boundary(prefix_len) {
        return None;
    }
    value[..prefix_len]
        .eq_ignore_ascii_case(prefix)
        .then_some(&value[prefix_len..])
}

fn strip_suffix_ci<'a>(value: &'a str, suffix: &str) -> Option<&'a str> {
    let suffix_len = suffix.len();
    if value.len() < suffix_len {
        return None;
    }
    let split = value.len() - suffix_len;
    if !value.is_char_boundary(split) {
        return None;
    }
    value[split..]
        .eq_ignore_ascii_case(suffix)
        .then_some(&value[..split])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_node(index: usize, role: &str, title: &str, tree_path: &[usize]) -> AxNode {
        AxNode {
            index,
            tree_path: tree_path.to_vec(),
            element_hash: None,
            role: Some(role.to_string()),
            identifier: None,
            title: Some(title.to_string()),
            value: None,
            description: None,
            placeholder: None,
            enabled: Some(true),
            settable_value: false,
            bounds: None,
            text: title.to_lowercase(),
            within_zoom_meeting_scope: true,
            within_zoom_chat_scope: false,
            within_slack_huddle_scope: false,
        }
    }

    #[test]
    fn extracts_zoom_video_tiles_and_participants_panel_without_self_or_chrome() {
        let nodes = vec![
            test_node(0, "AXWindow", "Q4 Roadmap Review - Zoom", &[]),
            test_node(
                1,
                "AXGroup",
                "Video render Ada Lovelace, Computer audio unmuted",
                &[0],
            ),
            test_node(2, "AXTabGroup", "John Jeong, Computer audio muted", &[1]),
            test_node(
                3,
                "AXGroup",
                "Stuart Chuang (Host, me), Computer audio unmuted",
                &[2],
            ),
            test_node(4, "AXGroup", "You are viewing Grace Hopper's screen", &[3]),
            test_node(5, "AXTable", "Participants (4)", &[4]),
            test_node(6, "AXRow", "Ada Lovelace (Co-host), Unmuted", &[4, 0]),
            test_node(7, "AXRow", "Linus Torvalds, Muted, Video off", &[4, 1]),
            test_node(8, "AXButton", "Mute All", &[4, 2]),
            test_node(9, "AXButton", "Leave", &[5]),
        ];

        assert_eq!(
            extract_meeting_title(
                &MeetingPlatform::Zoom,
                Some("Q4 Roadmap Review - Zoom"),
                &nodes
            ),
            Some("Q4 Roadmap Review".to_string())
        );
        assert_eq!(
            extract_meeting_participants(&MeetingPlatform::Zoom, &nodes),
            vec![
                "Ada Lovelace".to_string(),
                "John Jeong".to_string(),
                "Grace Hopper".to_string(),
                "Linus Torvalds".to_string(),
            ]
        );
    }

    #[test]
    fn skips_generic_zoom_window_title_and_reads_topic_label() {
        let nodes = vec![
            test_node(0, "AXWindow", "Zoom Meeting", &[]),
            test_node(1, "AXStaticText", "Meeting Topic: Weekly Design Sync", &[0]),
        ];

        assert_eq!(
            extract_meeting_title(&MeetingPlatform::Zoom, Some("Zoom Meeting"), &nodes),
            Some("Weekly Design Sync".to_string())
        );
        assert_eq!(
            extract_meeting_title(&MeetingPlatform::Zoom, Some("Zoom Meeting"), &[]),
            None
        );
    }

    #[test]
    fn cleans_google_meet_and_teams_titles_and_participants() {
        assert_eq!(
            extract_meeting_title(
                &MeetingPlatform::GoogleMeet,
                Some("Meet – abc-defg-hij - Google Chrome"),
                &[]
            ),
            None
        );
        assert_eq!(
            extract_meeting_title(
                &MeetingPlatform::GoogleMeet,
                Some("Meet – Product Review - Google Chrome"),
                &[]
            ),
            Some("Product Review".to_string())
        );
        assert_eq!(
            extract_meeting_title(
                &MeetingPlatform::MicrosoftTeams,
                Some("Architecture Sync | Microsoft Teams"),
                &[]
            ),
            Some("Architecture Sync".to_string())
        );

        let meet_nodes = vec![
            test_node(0, "AXGroup", "Priya Nair is presenting", &[0]),
            test_node(1, "AXGroup", "Sam Altman, microphone off", &[1]),
            test_node(2, "AXGroup", "Stuart (You), microphone on", &[2]),
        ];
        assert_eq!(
            extract_meeting_participants(&MeetingPlatform::GoogleMeet, &meet_nodes),
            vec!["Priya Nair".to_string(), "Sam Altman".to_string()]
        );
    }
}
