//! What Hold the Swarm remembers between runs: today's best, the best ever, and how many days in a row you've
//! played, as `key=value` lines in `~/.sphere/app/game`.
use super::engine::Day;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    /// the day `best` was scored (`2026-10-20`)
    pub day: String,
    pub best: u32,
    /// the best ever
    pub top: u32,
    pub streak: u32,
    /// the last day a run was played
    pub last: String,
}

impl Record {
    pub fn parse(s: &str) -> Record {
        let mut r = Record::default();
        for line in s.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "day" => r.day = v.to_string(),
                "best" => r.best = v.parse().unwrap_or(0),
                "top" => r.top = v.parse().unwrap_or(0),
                "streak" => r.streak = v.parse().unwrap_or(0),
                "last" => r.last = v.to_string(),
                _ => {}
            }
        }
        r
    }

    pub fn text(&self) -> String {
        format!("day={}\nbest={}\ntop={}\nstreak={}\nlast={}\n", self.day, self.best, self.top, self.streak, self.last)
    }

    /// Today's best (0 before the first run today).
    pub fn best_today(&self, day: &Day) -> u32 {
        if self.day == day.key {
            self.best
        } else {
            0
        }
    }

    /// The streak as it stands today: it holds while yesterday was played.
    pub fn streak_now(&self, day: &Day) -> u32 {
        if self.last == day.key || self.last == Day::of(day.n - 1).key {
            self.streak
        } else {
            0
        }
    }

    /// A run ended today with `score`. True when it is today's new best.
    pub fn played(&mut self, day: &Day, score: u32) -> bool {
        let best = score > self.best_today(day);
        if best {
            self.day = day.key.clone();
            self.best = score;
        }
        self.top = self.top.max(score);
        if self.last != day.key {
            self.streak = if self.last == Day::of(day.n - 1).key { self.streak + 1 } else { 1 };
            self.last = day.key.clone();
        }
        best
    }
}

fn file() -> std::path::PathBuf {
    sphere_proto::paths::default_state_dir().join("app").join("game")
}

pub fn load() -> Record {
    std::fs::read_to_string(file()).map(|s| Record::parse(&s)).unwrap_or_default()
}

pub fn save(r: &Record) {
    let f = file();
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, r.text());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_reads_back_what_it_wrote() {
        let r = Record { day: "2026-10-20".into(), best: 1234, top: 5678, streak: 3, last: "2026-10-20".into() };
        assert_eq!(Record::parse(&r.text()), r);
        assert_eq!(Record::parse("nonsense\nbest=x\n"), Record::default());
    }

    #[test]
    fn the_best_is_today_s_and_the_streak_counts_days_in_a_row() {
        let d1 = Day::first();
        let d2 = Day::of(d1.n + 1);
        let d4 = Day::of(d1.n + 3);
        let mut r = Record::default();
        assert!(r.played(&d1, 500));
        assert!(!r.played(&d1, 400));
        assert_eq!((r.best_today(&d1), r.streak_now(&d1)), (500, 1));
        // a new day starts its best at nothing; playing it makes two in a row
        assert_eq!(r.best_today(&d2), 0);
        assert!(r.played(&d2, 100));
        assert_eq!((r.streak, r.top), (2, 500));
        assert!(!r.played(&d2, 0));
        // a day missed: the streak shows nothing, then starts again
        assert_eq!(r.streak_now(&d4), 0);
        r.played(&d4, 10);
        assert_eq!(r.streak, 1);
    }
}
