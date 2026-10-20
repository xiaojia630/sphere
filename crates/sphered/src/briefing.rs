//! The briefing: the user's notes for a repo as a block of text for an agent's system prompt.

use sphere_proto::Note;

/// The most the briefing may take, in bytes. Notes that don't fit are left out.
pub const BRIEFING_MAX_BYTES: usize = 4096;

const HEADER: &str =
    "Notes from the user for this repository. Follow them unless the task says otherwise:";

/// What an agent is told, and how many notes that is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Briefing {
    pub text: String,
    pub notes: u32,
}

fn newest_first(mut group: Vec<&Note>) -> Vec<&Note> {
    group.sort_by_key(|n| std::cmp::Reverse(n.id));
    group
}

/// What to tell an agent working in `repo`: the repo's notes, then the ones for every repo, newest first in each.
/// When they don't all fit in [`BRIEFING_MAX_BYTES`], the end of that order is left out: the oldest of the notes
/// for every repo go first. `None` when no note applies.
pub fn briefing(notes: &[Note], repo: &str) -> Option<Briefing> {
    let own = newest_first(
        notes
            .iter()
            .filter(|n| n.repo.as_deref() == Some(repo))
            .collect(),
    );
    let all = newest_first(notes.iter().filter(|n| n.repo.is_none()).collect());
    let mut text = String::from(HEADER);
    let mut count = 0;
    for note in own.into_iter().chain(all) {
        let line = format!("\n- {}", note.text);
        if text.len() + line.len() > BRIEFING_MAX_BYTES {
            break;
        }
        text.push_str(&line);
        count += 1;
    }
    (count > 0).then_some(Briefing { text, notes: count })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sphere_proto::NoteSource;

    fn note(id: u64, repo: Option<&str>, text: &str) -> Note {
        Note {
            id,
            repo: repo.map(str::to_string),
            text: text.into(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            source: NoteSource::Typed,
        }
    }

    #[test]
    fn no_notes_for_the_repo_means_no_briefing() {
        assert_eq!(briefing(&[], "/r"), None);
        assert_eq!(briefing(&[note(1, Some("/other"), "x")], "/r"), None);
    }

    #[test]
    fn the_repos_notes_come_first_then_the_ones_for_every_repo_newest_first() {
        let notes = [
            note(1, Some("/r"), "use pnpm, never npm"),
            note(2, None, "answer in plain words"),
            note(3, Some("/r"), "tests live in spec/"),
            note(4, Some("/other"), "not for this repo"),
            note(5, None, "keep commits small"),
        ];
        let b = briefing(&notes, "/r").unwrap();
        assert_eq!(
            b.text,
            "Notes from the user for this repository. Follow them unless the task says otherwise:\n\
             - tests live in spec/\n\
             - use pnpm, never npm\n\
             - keep commits small\n\
             - answer in plain words"
        );
        assert_eq!(b.notes, 4);
    }

    #[test]
    fn the_order_does_not_depend_on_how_the_notes_arrive() {
        let a = [note(1, Some("/r"), "old"), note(2, Some("/r"), "new")];
        let b = [note(2, Some("/r"), "new"), note(1, Some("/r"), "old")];
        assert_eq!(briefing(&a, "/r"), briefing(&b, "/r"));
        assert!(briefing(&a, "/r").unwrap().text.ends_with("- new\n- old"));
    }

    #[test]
    fn notes_for_every_repo_alone_brief_every_repo() {
        let b = briefing(&[note(1, None, "be brief")], "/anywhere").unwrap();
        assert_eq!(b.notes, 1);
        assert!(b.text.ends_with("\n- be brief"));
    }

    #[test]
    fn the_briefing_stops_at_the_cap_and_leaves_out_the_oldest() {
        // 300-character notes, the most a note can be: 13 of them fit under 4 KB and the 14th would not
        let text = |i: usize| format!("{i:03}{}", "x".repeat(297));
        let notes: Vec<Note> = (1..=20)
            .map(|i| note(i as u64, Some("/r"), &text(i)))
            .collect();
        let b = briefing(&notes, "/r").unwrap();
        assert!(b.text.len() <= BRIEFING_MAX_BYTES, "{}", b.text.len());
        assert_eq!(b.notes, 13);
        assert!(b.text.contains(&text(20)) && b.text.contains(&text(8)));
        assert!(!b.text.contains(&text(7)) && !b.text.contains(&text(1)));
    }

    #[test]
    fn the_oldest_of_the_notes_for_every_repo_go_before_the_repos_own() {
        let big = |i: usize| format!("{i:03}{}", "y".repeat(297));
        let mut notes: Vec<Note> = (1..=10)
            .map(|i| note(i as u64, Some("/r"), &big(i)))
            .collect();
        // newer than every note of the repo, and for every repo
        notes.extend((11..=20).map(|i| note(i as u64, None, &big(i))));
        let b = briefing(&notes, "/r").unwrap();
        assert!(b.text.len() <= BRIEFING_MAX_BYTES);
        // all ten of the repo's own are in, and what is left of the room goes to the newest of the others
        assert_eq!(b.notes, 13);
        for i in (1..=10).chain(18..=20) {
            assert!(b.text.contains(&big(i)), "note {i}");
        }
        assert!(!b.text.contains(&big(17)) && !b.text.contains(&big(11)));
    }
}
