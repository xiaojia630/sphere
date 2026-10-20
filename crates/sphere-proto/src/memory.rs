//! Notes: what the user wants every agent in a repo to know, like "use pnpm, never npm". Sphere tells them to
//! each agent it starts there. sphered keeps them in its store, apart from the event log, so a removed note
//! is really gone.

use crate::event::now_rfc3339;
use crate::views::seconds_between;
use serde::{Deserialize, Serialize};

/// The longest a note can be, in characters.
pub const NOTE_MAX_CHARS: usize = 300;

/// How many notes sphered keeps in all.
pub const NOTES_MAX: usize = 200;

/// Where a note came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteSource {
    /// Written in the command bar.
    #[default]
    Typed,
    /// The text of a Change the user sent, kept with one key.
    Change,
    /// Added with `sphere remember`.
    Cli,
}

impl NoteSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            NoteSource::Typed => "typed",
            NoteSource::Change => "change",
            NoteSource::Cli => "cli",
        }
    }

    /// The source named by `as_str`; anything else counts as typed.
    pub fn parse(s: &str) -> NoteSource {
        match s {
            "change" => NoteSource::Change,
            "cli" => NoteSource::Cli,
            _ => NoteSource::Typed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// Never reused, so an id from a list still names the same note when it is removed.
    pub id: u64,
    /// The repo it is for, as a task's `repo` is (the root of the checkout). `None`: every repo.
    #[serde(default)]
    pub repo: Option<String>,
    pub text: String,
    /// RFC 3339, UTC, as in `Event::at`.
    pub created_at: String,
    pub source: NoteSource,
}

impl Note {
    /// Whether an agent working in `repo` is told this note.
    pub fn applies_to(&self, repo: &str) -> bool {
        self.repo.as_deref().is_none_or(|r| r == repo)
    }

    /// Whole seconds since it was written.
    pub fn age_secs(&self) -> u64 {
        seconds_between(&self.created_at, &now_rfc3339())
    }
}

/// The notes by repo, for a list: each repo with its notes, the repos in the order of their newest note, then the
/// notes for every repo (`None`) last. Newest first inside each.
pub fn by_repo(notes: &[Note]) -> Vec<(Option<&str>, Vec<&Note>)> {
    let mut sorted: Vec<&Note> = notes.iter().collect();
    sorted.sort_by_key(|n| std::cmp::Reverse(n.id));
    let mut groups: Vec<(Option<&str>, Vec<&Note>)> = vec![];
    for n in sorted {
        match groups.iter_mut().find(|(r, _)| *r == n.repo.as_deref()) {
            Some((_, g)) => g.push(n),
            None => groups.push((n.repo.as_deref(), vec![n])),
        }
    }
    // stable: the repos keep the order of their newest note
    groups.sort_by_key(|(r, _)| r.is_none());
    groups
}

/// A repo's name for a title: the last folder of its path.
pub fn repo_name(repo: &str) -> &str {
    let trimmed = repo.trim_end_matches('/');
    trimmed
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or(repo)
}

/// A short age for a list: `just now`, `5m ago`, `3h ago`, `2d ago`.
pub fn ago(secs: u64) -> String {
    match secs {
        0..90 => "just now".into(),
        90..5400 => format!("{}m ago", secs / 60),
        5400..129_600 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

/// A note as it is kept: one line with single spaces, at most [`NOTE_MAX_CHARS`] characters (a longer one is cut
/// and ends in `…`). `None` when nothing is left of it.
pub fn clean_note(text: &str) -> Option<String> {
    let text: String = text
        .chars()
        .filter(|c| !c.is_control() || c.is_whitespace())
        .collect();
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return None;
    }
    if line.chars().count() <= NOTE_MAX_CHARS {
        return Some(line);
    }
    let mut cut: String = line.chars().take(NOTE_MAX_CHARS - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    Some(cut)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note_with(id: u64, repo: Option<&str>, text: &str) -> Note {
        Note {
            id,
            repo: repo.map(str::to_string),
            text: text.into(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            source: NoteSource::Typed,
        }
    }

    fn note(repo: Option<&str>) -> Note {
        note_with(1, repo, "use pnpm, never npm")
    }

    #[test]
    fn a_note_is_one_line_with_single_spaces() {
        assert_eq!(
            clean_note("  use pnpm,\n\tnever   npm \r\n").as_deref(),
            Some("use pnpm, never npm")
        );
        assert_eq!(clean_note("a\u{7}b\u{1b}[0m c").as_deref(), Some("ab[0m c"));
    }

    #[test]
    fn a_note_with_nothing_in_it_is_nothing() {
        assert_eq!(clean_note(""), None);
        assert_eq!(clean_note(" \n\t "), None);
        assert_eq!(clean_note("\u{0}\u{7}"), None);
    }

    #[test]
    fn a_long_note_is_cut_to_the_cap_on_a_character_boundary() {
        let exact = "é".repeat(NOTE_MAX_CHARS);
        assert_eq!(clean_note(&exact).as_deref(), Some(exact.as_str()));
        let long = format!("{} tail", "é".repeat(NOTE_MAX_CHARS));
        let cut = clean_note(&long).unwrap();
        assert_eq!(cut.chars().count(), NOTE_MAX_CHARS);
        assert!(cut.ends_with('…') && !cut.contains("tail"));
        // the space the cut lands after is dropped, not left before the mark
        let spaced = format!("{} {}", "a".repeat(NOTE_MAX_CHARS - 2), "b".repeat(50));
        let cut = clean_note(&spaced).unwrap();
        assert!(cut.ends_with("a…"), "{cut}");
    }

    #[test]
    fn a_note_applies_to_its_repo_and_to_all_repos() {
        assert!(note(Some("/code/sphere")).applies_to("/code/sphere"));
        assert!(!note(Some("/code/sphere")).applies_to("/code/other"));
        assert!(note(None).applies_to("/code/other"));
    }

    #[test]
    fn notes_are_grouped_by_repo_newest_first_with_every_repo_last() {
        let n = |id, repo: Option<&str>| note_with(id, repo, "x");
        let notes = [
            n(1, Some("/a")),
            n(2, None),
            n(3, Some("/b")),
            n(4, Some("/a")),
            n(5, None),
        ];
        let groups = by_repo(&notes);
        let shape: Vec<(Option<&str>, Vec<u64>)> = groups
            .iter()
            .map(|(r, g)| (*r, g.iter().map(|n| n.id).collect()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (Some("/a"), vec![4, 1]),
                (Some("/b"), vec![3]),
                (None, vec![5, 2])
            ]
        );
        assert!(by_repo(&[]).is_empty());
    }

    #[test]
    fn a_repo_is_named_by_its_last_folder() {
        assert_eq!(repo_name("/Users/me/code/sphere"), "sphere");
        assert_eq!(repo_name("/Users/me/code/sphere/"), "sphere");
        assert_eq!(repo_name("/"), "/");
        assert_eq!(repo_name("sphere"), "sphere");
    }

    #[test]
    fn ages_are_short() {
        let said: Vec<String> = [0, 89, 90, 5399, 5400, 129_599, 129_600, 900_000]
            .into_iter()
            .map(ago)
            .collect();
        assert_eq!(
            said,
            [
                "just now", "just now", "1m ago", "89m ago", "1h ago", "35h ago", "1d ago",
                "10d ago"
            ]
        );
    }

    #[test]
    fn a_note_knows_how_old_it_is() {
        let mut n = note(Some("/r"));
        assert!(n.age_secs() > 86400 * 30, "written in January 2026");
        n.created_at = now_rfc3339();
        assert!(n.age_secs() < 5);
        n.created_at = "not a time".into();
        assert_eq!(n.age_secs(), 0);
    }

    #[test]
    fn a_note_round_trips_as_json() {
        let n = Note {
            source: NoteSource::Change,
            ..note(Some("/code/sphere"))
        };
        let v = serde_json::to_value(&n).unwrap();
        assert_eq!(v["source"], "change");
        assert_eq!(serde_json::from_value::<Note>(v).unwrap(), n);
        let all: Note = serde_json::from_str(
            r#"{"id":2,"text":"x","created_at":"2026-01-01T00:00:00.000Z","source":"cli"}"#,
        )
        .unwrap();
        assert_eq!((all.repo, all.source), (None, NoteSource::Cli));
    }

    #[test]
    fn sources_are_named_and_read_back() {
        for s in [NoteSource::Typed, NoteSource::Change, NoteSource::Cli] {
            assert_eq!(NoteSource::parse(s.as_str()), s);
        }
        assert_eq!(NoteSource::parse("from_the_future"), NoteSource::Typed);
    }
}
