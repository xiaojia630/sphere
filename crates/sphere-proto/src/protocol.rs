//! Protocols: agents on your schedule. A routine is one typed line, `every weekday 9:00 sphere: update the
//! dependencies`, and Sphere runs it as an ordinary task whenever it comes due, with the app open or not.
//!
//! This is what sphered, the CLI and the app share: reading the line, working out when it runs next, and the words
//! for both. The clock and the time zone come in as arguments, so the three agree and a test never waits for either.

use crate::event::TaskState;
use serde::{Deserialize, Serialize};
use std::fmt;

const DAY: i64 = 86_400;

/// The shortest gap between two runs of a repeating protocol, in minutes.
pub const MIN_EVERY_MINUTES: u32 = 15;
/// The longest: for rarer runs a weekday schedule says it better.
const MAX_EVERY_MINUTES: u32 = 7 * 24 * 60;

/// What the line should look like, for the errors that don't have anything better to say.
const EXAMPLE: &str = "every weekday 9:00 repo: what to do";

// ---- the clock ------------------------------------------------------------------------------------------------------

/// The local time zone, as far as schedules need it.
pub trait Zone {
    /// Seconds the zone is ahead of UTC at the Unix time `utc`. It changes with daylight saving.
    fn offset(&self, utc: i64) -> i64;

    /// The Unix time of a wall-clock reading, given as seconds since 1970-01-01 00:00 read as if it were UTC.
    /// A reading the clocks pass twice (they go back) means the first time. One they skip (they go forward) is read
    /// with the offset from before the change, so it falls just after the gap.
    fn to_utc(&self, local: i64) -> i64 {
        // The offsets a day either side bracket any change near the reading.
        let before = self.offset(local - DAY);
        let after = self.offset(local + DAY);
        let (early, late) = (local - before, local - after);
        match (self.offset(early) == before, self.offset(late) == after) {
            (true, true) => early.min(late),
            (false, true) => late,
            _ => early,
        }
    }
}

/// A zone that is always the same number of seconds ahead of UTC.
#[derive(Debug, Clone, Copy)]
pub struct FixedZone(pub i64);

impl Zone for FixedZone {
    fn offset(&self, _utc: i64) -> i64 {
        self.0
    }
}

/// This machine's own zone, daylight saving included.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalZone;

impl Zone for LocalZone {
    #[cfg(target_arch = "wasm32")]
    fn offset(&self, utc: i64) -> i64 {
        // minutes behind UTC, so east of Greenwich is negative
        -(js_sys::Date::new(&js_sys::wasm_bindgen::JsValue::from_f64(
            utc as f64 * 1000.0,
        ))
        .get_timezone_offset() as i64)
            * 60
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn offset(&self, utc: i64) -> i64 {
        extern "C" {
            fn tzset();
        }
        // SAFETY: tzset takes no pointers (and notices a zone that changed since the last call); localtime_r only
        // reads `t` and writes the zeroed `tm` we give it.
        unsafe {
            tzset();
            let t: libc::time_t = utc as libc::time_t;
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&t, &mut tm).is_null() {
                0
            } else {
                tm.tm_gmtoff as i64
            }
        }
    }
}

/// Days since 1970-01-01 of a calendar date (proleptic Gregorian). Tests build their moments with it.
#[cfg(test)]
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * i64::from(if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The calendar date of a count of days since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// The local day (days since 1970-01-01) and the seconds into it at the Unix time `utc`.
fn local_parts(utc: i64, zone: &dyn Zone) -> (i64, i64) {
    let local = utc + zone.offset(utc);
    (local.div_euclid(DAY), local.rem_euclid(DAY))
}

/// When the local clock reads `hour:minute` on the local day `day`.
fn at_local(day: i64, hour: u8, minute: u8, zone: &dyn Zone) -> i64 {
    zone.to_utc(day * DAY + i64::from(hour) * 3600 + i64::from(minute) * 60)
}

/// The next time after `now` the local clock reads `hour:minute`.
fn next_clock(now: i64, hour: u8, minute: u8, zone: &dyn Zone) -> i64 {
    let (today, _) = local_parts(now, zone);
    let t = at_local(today, hour, minute, zone);
    if t > now {
        t
    } else {
        at_local(today + 1, hour, minute, zone)
    }
}

// ---- the schedule ---------------------------------------------------------------------------------------------------

/// A day of the week, Monday first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Weekday {
    pub const ALL: [Weekday; 7] = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ];
    pub const WEEKDAYS: [Weekday; 5] = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
    ];
    pub const WEEKEND: [Weekday; 2] = [Weekday::Sat, Weekday::Sun];

    /// `Mon`.
    pub fn short(self) -> &'static str {
        ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][self as usize]
    }

    /// `Monday`.
    pub fn long(self) -> &'static str {
        [
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ][self as usize]
    }

    /// The weekday of a count of days since 1970-01-01, which was a Thursday.
    fn of_days(days: i64) -> Weekday {
        Weekday::ALL[(days + 3).rem_euclid(7) as usize]
    }
}

/// When a protocol runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Schedule {
    /// On these days when the local clock reads `hour:minute`: `every weekday 9:00`, `daily 9:00`,
    /// `every mon,wed,fri 18:30`.
    Weekly {
        days: Vec<Weekday>,
        hour: u8,
        minute: u8,
    },
    /// Every so many minutes, counted from `from` (Unix seconds, when it was written): `every hour`,
    /// `every 30 minutes`. Counting from one point keeps the beat steady however late a run starts.
    Every { minutes: u32, from: i64 },
    /// Once, at this moment (Unix seconds), worked out when the line was written: `at 18:00`, `tomorrow 9:00`,
    /// `tonight`, `in 2 hours`.
    Once { at: i64 },
}

impl Schedule {
    /// The first run strictly after `after` (Unix seconds). None for a one-shot that has passed.
    pub fn next_after(&self, after: i64, zone: &dyn Zone) -> Option<i64> {
        match self {
            Schedule::Once { at } => (*at > after).then_some(*at),
            Schedule::Every { minutes, from } => {
                let step = i64::from(*minutes).max(1) * 60;
                let n = ((after - from).div_euclid(step) + 1).max(1);
                Some(from + n * step)
            }
            Schedule::Weekly { days, hour, minute } => {
                let (today, _) = local_parts(after, zone);
                // A week and a day more: the clocks may skip the first morning's time.
                (0..=8).find_map(|ahead| {
                    let day = today + ahead;
                    if !days.contains(&Weekday::of_days(day)) {
                        return None;
                    }
                    Some(at_local(day, *hour, *minute, zone)).filter(|t| *t > after)
                })
            }
        }
    }

    pub fn is_one_shot(&self) -> bool {
        matches!(self, Schedule::Once { .. })
    }

    /// The schedule in words, as the line says it: `every weekday 09:00`, `every mon, wed, fri 18:30`,
    /// `every 2 hours`. A one-shot reads `once · Tue 09:00`.
    pub fn describe(&self, now: i64, zone: &dyn Zone) -> String {
        match self {
            Schedule::Every { minutes, .. } => every_words(*minutes),
            Schedule::Once { at } => format!("once · {}", when_words(*at, now, zone)),
            Schedule::Weekly { days, hour, minute } => {
                let which = if days.len() == 7 {
                    "day".to_string()
                } else if days[..] == Weekday::WEEKDAYS {
                    "weekday".to_string()
                } else if days[..] == Weekday::WEEKEND {
                    "weekend".to_string()
                } else if let [one] = days.as_slice() {
                    one.long().to_lowercase()
                } else {
                    days.iter()
                        .map(|d| d.short().to_lowercase())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                format!("every {which} {hour:02}:{minute:02}")
            }
        }
    }
}

/// `every hour`, `every 2 hours`, `every 30 minutes`.
fn every_words(minutes: u32) -> String {
    match minutes {
        1 => "every minute".to_string(),
        60 => "every hour".to_string(),
        m if m % 60 == 0 => format!("every {} hours", m / 60),
        m => format!("every {m} minutes"),
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A moment in local time, short: `09:00` today, `Mon 09:00` within the week, `Mon 3 Nov 09:00` beyond (with the
/// year when it is not this one).
pub fn when_words(t: i64, now: i64, zone: &dyn Zone) -> String {
    let (day, secs) = local_parts(t, zone);
    let (today, _) = local_parts(now, zone);
    let clock = format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60);
    let ahead = day - today;
    if ahead == 0 {
        return clock;
    }
    let weekday = Weekday::of_days(day).short();
    if (1..7).contains(&ahead) {
        return format!("{weekday} {clock}");
    }
    let (y, m, d) = civil_from_days(day);
    let month = MONTHS[m as usize - 1];
    if civil_from_days(today).0 == y {
        format!("{weekday} {d} {month} {clock}")
    } else {
        format!("{weekday} {d} {month} {y} {clock}")
    }
}

/// A stretch of time in a few characters: `<1 min`, `12 min`, `3 h`, `2 d`.
pub fn span_words(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => "<1 min".to_string(),
        s if s < 3600 => format!("{} min", s / 60),
        s if s < DAY => format!("{} h", s / 3600),
        s => format!("{} d", s / DAY),
    }
}

// ---- reading the line -----------------------------------------------------------------------------------------------

/// What a protocol line says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub schedule: Schedule,
    /// The repo as typed (a folder name, `~/x` or a path); None when the line names none.
    pub repo: Option<String>,
    pub prompt: String,
}

/// Why a line is not a protocol, in words that say what to type instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

fn err(message: impl Into<String>) -> ParseError {
    ParseError(message.into())
}

/// One word of the line, lower-cased. A colon stuck to the end of a word is not part of it: it marks where the
/// schedule stops.
struct Word {
    text: String,
    /// Where the word ends in the line, before any colon.
    end: usize,
    colon: bool,
}

fn words(line: &str) -> Vec<Word> {
    let mut out = vec![];
    let mut at = 0;
    while let Some(skip) = line[at..].find(|c: char| !c.is_whitespace()) {
        let start = at + skip;
        let len = line[start..]
            .find(char::is_whitespace)
            .unwrap_or(line.len() - start);
        let raw = &line[start..start + len];
        let (text, colon) = match raw.strip_suffix(':') {
            Some(text) => (text, true),
            None => (raw, false),
        };
        out.push(Word {
            text: text.to_lowercase(),
            end: start + text.len(),
            colon,
        });
        at = start + len;
    }
    out
}

/// A reader over the words of the schedule.
struct Cursor<'a> {
    words: &'a [Word],
    at: usize,
}

impl<'a> Cursor<'a> {
    /// The next word, unless a colon ended the schedule before it.
    fn peek(&self) -> Option<&'a str> {
        if self.at > 0 && self.words[self.at - 1].colon {
            return None;
        }
        self.words.get(self.at).map(|w| w.text.as_str())
    }

    fn next(&mut self) -> Option<&'a str> {
        let w = self.peek()?;
        self.at += 1;
        Some(w)
    }

    /// Step over `word` when it is next.
    fn skip(&mut self, word: &str) {
        if self.peek() == Some(word) {
            self.at += 1;
        }
    }

    /// Where the words read so far end in the line.
    fn end(&self) -> usize {
        self.words[self.at - 1].end
    }

    /// A time of day: `9`, `9:00`, `18:30`, `9am`, `6:30pm`, `9 pm`.
    fn time(&mut self) -> Result<(u8, u8), ParseError> {
        let which = || err(format!("which time? try '{EXAMPLE}'"));
        let Some(w) = self
            .peek()
            .filter(|w| w.starts_with(|c: char| c.is_ascii_digit()))
        else {
            return Err(which());
        };
        self.at += 1;
        let mut text = w.to_string();
        if let Some(half) = self.peek().filter(|w| matches!(*w, "am" | "pm")) {
            text.push_str(half);
            self.at += 1;
        }
        clock(&text).ok_or_else(|| {
            err(format!(
                "didn't understand the time '{text}': try 9:00, 18:30 or 6pm"
            ))
        })
    }

    /// A length of time: `2 hours`, `45 minutes`, `1 hour`, `2h`, `30m`. Minutes.
    fn duration(&mut self, lead: &str) -> Result<u32, ParseError> {
        let ask = || {
            if lead == "in" {
                err("in how long? try 'in 2 hours' or 'in 45 minutes'")
            } else {
                err("every how long? try 'every 2 hours' or 'every 30 minutes'")
            }
        };
        let Some(w) = self
            .peek()
            .filter(|w| w.starts_with(|c: char| c.is_ascii_digit()))
        else {
            return Err(ask());
        };
        self.at += 1;
        let digits = w.chars().take_while(char::is_ascii_digit).count();
        let (number, glued) = w.split_at(digits);
        // `2h` carries its unit; `2 hours` has it in the next word.
        let unit = if glued.is_empty() {
            self.peek().unwrap_or("")
        } else {
            glued
        };
        let per = unit_minutes(unit);
        if per.is_some() && glued.is_empty() {
            self.at += 1;
        }
        match (number.parse::<u32>(), per) {
            (Ok(n), Some(per)) => n.checked_mul(per).ok_or_else(ask),
            (_, None) if unit.is_empty() => Err(err(format!(
                "{lead} {number} what? try '{lead} 2 hours' or '{lead} 30 minutes'"
            ))),
            _ => {
                let shown = if glued.is_empty() {
                    format!("{lead} {w} {unit}")
                } else {
                    format!("{lead} {w}")
                };
                Err(err(format!(
                    "didn't understand '{shown}': try '{lead} 2 hours' or '{lead} 30 minutes'"
                )))
            }
        }
    }
}

/// Minutes in a unit of time.
fn unit_minutes(unit: &str) -> Option<u32> {
    match unit {
        "m" | "min" | "mins" | "minute" | "minutes" => Some(1),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(60),
        _ => None,
    }
}

/// `9`, `9:00`, `09:00`, `18:30`, `9am`, `6pm`, `6:30pm`.
fn clock(text: &str) -> Option<(u8, u8)> {
    let (body, pm) = match (text.strip_suffix("am"), text.strip_suffix("pm")) {
        (Some(body), _) => (body, Some(false)),
        (_, Some(body)) => (body, Some(true)),
        _ => (text, None),
    };
    let (hour, minute) = body.split_once(':').unwrap_or((body, "00"));
    let digits = |s: &str, len: std::ops::RangeInclusive<usize>| {
        len.contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
    };
    if !digits(hour, 1..=2) || !digits(minute, 2..=2) {
        return None;
    }
    let (hour, minute): (u8, u8) = (hour.parse().ok()?, minute.parse().ok()?);
    if minute > 59 {
        return None;
    }
    match pm {
        None => (hour <= 23).then_some((hour, minute)),
        Some(pm) => (1..=12)
            .contains(&hour)
            .then_some((hour % 12 + if pm { 12 } else { 0 }, minute)),
    }
}

fn weekday(word: &str) -> Option<Weekday> {
    Some(match word {
        "mon" | "monday" | "mondays" => Weekday::Mon,
        "tue" | "tues" | "tuesday" | "tuesdays" => Weekday::Tue,
        "wed" | "weds" | "wednesday" | "wednesdays" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" | "thursdays" => Weekday::Thu,
        "fri" | "friday" | "fridays" => Weekday::Fri,
        "sat" | "saturday" | "saturdays" => Weekday::Sat,
        "sun" | "sunday" | "sundays" => Weekday::Sun,
        _ => return None,
    })
}

/// A day, or a range of days, such as `mon-fri` (which may wrap past Sunday).
fn day_span(part: &str) -> Option<Vec<Weekday>> {
    match part.split_once('-') {
        None => weekday(part).map(|d| vec![d]),
        Some((a, b)) => {
            let (a, b) = (weekday(a)? as usize, weekday(b)? as usize);
            let len = (b + 7 - a) % 7 + 1;
            Some((0..len).map(|i| Weekday::ALL[(a + i) % 7]).collect())
        }
    }
}

/// Days named the way a line names them: `mon`, `monday`, `mon,wed,fri`, `mon, wed and fri`, `mon-fri`.
fn day_list(c: &mut Cursor) -> Vec<Weekday> {
    let mut days = vec![];
    while let Some(w) = c.peek() {
        let parts: Vec<&str> = w.split(',').filter(|p| !p.is_empty()).collect();
        let spans: Option<Vec<Vec<Weekday>>> = parts.iter().map(|p| day_span(p)).collect();
        match spans {
            Some(spans) if !spans.is_empty() => days.extend(spans.into_iter().flatten()),
            _ => break,
        }
        c.at += 1;
        if w.ends_with(',') {
            continue;
        }
        if matches!(c.peek(), Some("and" | "&")) {
            c.at += 1;
            continue;
        }
        break;
    }
    days.sort();
    days.dedup();
    days
}

fn didnt_understand(word: &str) -> ParseError {
    err(format!("didn't understand '{word}': try '{EXAMPLE}'"))
}

/// What follows `every`.
fn every(c: &mut Cursor, now: i64) -> Result<Schedule, ParseError> {
    let Some(w) = c.peek() else {
        return Err(err(
            "every what? try 'every day 9:00', 'every mon,wed 18:30' or 'every 2 hours'",
        ));
    };
    let days = match w {
        "hour" => {
            c.at += 1;
            return Ok(Schedule::Every {
                minutes: 60,
                from: now,
            });
        }
        "minute" => {
            c.at += 1;
            return Err(too_often(1));
        }
        "day" | "days" => Weekday::ALL.to_vec(),
        "weekday" | "weekdays" => Weekday::WEEKDAYS.to_vec(),
        "weekend" | "weekends" => Weekday::WEEKEND.to_vec(),
        w if w.starts_with(|ch: char| ch.is_ascii_digit()) => {
            let minutes = c.duration("every")?;
            if minutes < MIN_EVERY_MINUTES {
                return Err(too_often(minutes));
            }
            if minutes > MAX_EVERY_MINUTES {
                return Err(err(
                    "that is more than a week apart: try a day, like 'every monday 9:00'",
                ));
            }
            return Ok(Schedule::Every { minutes, from: now });
        }
        _ => {
            let days = day_list(c);
            if days.is_empty() {
                return Err(err(format!("didn't understand 'every {w}': try 'every day 9:00', 'every mon,wed 18:30' or 'every 2 hours'")));
            }
            let (hour, minute) = at_time(c)?;
            return Ok(Schedule::Weekly { days, hour, minute });
        }
    };
    c.at += 1;
    let (hour, minute) = at_time(c)?;
    Ok(Schedule::Weekly { days, hour, minute })
}

/// `9:00` or `at 9:00`.
fn at_time(c: &mut Cursor) -> Result<(u8, u8), ParseError> {
    c.skip("at");
    c.time()
}

fn too_often(minutes: u32) -> ParseError {
    err(format!(
        "{} is too often: the minimum is every {MIN_EVERY_MINUTES} minutes",
        every_words(minutes)
    ))
}

/// Read a protocol line: `<schedule> [<repo>]: <prompt>`, such as
/// `every weekday 9:00 sphere: update the dependencies, run the tests and fix what breaks`.
///
/// `now` is the Unix time the line is written at: one-shots and `every N minutes` count from it. The prompt starts
/// after the first colon that follows the schedule, so the colon of a time (`9:00`) is not mistaken for it.
pub fn parse(line: &str, now: i64, zone: &dyn Zone) -> Result<Parsed, ParseError> {
    let ws = words(line);
    let mut c = Cursor { words: &ws, at: 0 };
    let Some(first) = c.next() else {
        return Err(err(format!(
            "write when it runs, then what to do: '{EXAMPLE}'"
        )));
    };
    let schedule = match first {
        "every" => every(&mut c, now)?,
        "daily" => {
            let (hour, minute) = at_time(&mut c)?;
            Schedule::Weekly {
                days: Weekday::ALL.to_vec(),
                hour,
                minute,
            }
        }
        "at" => {
            let (hour, minute) = c.time()?;
            Schedule::Once {
                at: next_clock(now, hour, minute, zone),
            }
        }
        "tomorrow" => {
            let (hour, minute) = at_time(&mut c)?;
            let (today, _) = local_parts(now, zone);
            Schedule::Once {
                at: at_local(today + 1, hour, minute, zone),
            }
        }
        "tonight" => Schedule::Once {
            at: next_clock(now, 2, 0, zone),
        },
        "in" => {
            let minutes = c.duration("in")?;
            if minutes == 0 {
                return Err(err("in how long? try 'in 2 hours' or 'in 45 minutes'"));
            }
            Schedule::Once {
                at: now + i64::from(minutes) * 60,
            }
        }
        other => return Err(didnt_understand(other)),
    };
    let rest = &line[c.end()..];
    let Some((repo, prompt)) = rest.split_once(':') else {
        return Err(err(format!(
            "what should it do? add a colon and the task: '{EXAMPLE}'"
        )));
    };
    let repo = repo.trim();
    if repo.contains(char::is_whitespace) {
        return Err(err(format!(
            "didn't understand '{repo}' before the colon: put one repo name there, like 'every weekday 9:00 sphere: what to do'"
        )));
    }
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(err(format!(
            "what should it do? write it after the colon: '{EXAMPLE}'"
        )));
    }
    Ok(Parsed {
        schedule,
        repo: (!repo.is_empty()).then(|| repo.to_string()),
        prompt: prompt.to_string(),
    })
}

/// What a line typed in the command bar is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// Not about a schedule: an ordinary task.
    Task,
    Protocol(Parsed),
    /// Starts like a schedule but is not one; the words say what to fix.
    Mistake(ParseError),
}

/// Sort out a line from the command bar. A task that merely starts with `every` or `in`, such as
/// `every test in the repo needs a comment: add them`, stays a task: only a line that starts with an actual schedule
/// (and so would have been one had it read right) is a protocol or a mistake.
pub fn classify(line: &str, now: i64, zone: &dyn Zone) -> Line {
    match parse(line, now, zone) {
        Ok(p) => Line::Protocol(p),
        Err(e) if starts_like_schedule(line) => Line::Mistake(e),
        Err(_) => Line::Task,
    }
}

/// Does the line open with something only a schedule says: `every day`, `every mon`, `every 2 hours`, `daily 9:00`,
/// `at 18:00`, `tomorrow 9`, `tonight sphere:`, `in 2 hours`?
fn starts_like_schedule(line: &str) -> bool {
    let ws = words(line);
    let word = |i: usize| ws.get(i).map(|w| w.text.as_str());
    // How many words come before the colon that ends the schedule and repo part.
    let head = ws.iter().position(|w| w.colon).map_or(ws.len(), |i| i + 1);
    // A time with a colon or am/pm is plainly a time; a bare number only when little else is said.
    let time = |w: Option<&str>| {
        w.is_some_and(|w| {
            w.starts_with(|c: char| c.is_ascii_digit())
                && (w.contains(':')
                    || w.ends_with("am")
                    || w.ends_with("pm")
                    || (w.len() <= 2 && head <= 3))
        })
    };
    let duration = |i: usize| {
        word(i).is_some_and(|w| {
            let digits = w.chars().take_while(char::is_ascii_digit).count();
            digits > 0
                && (unit_minutes(&w[digits..]).is_some()
                    || (digits == w.len()
                        && word(i + 1).is_some_and(|u| unit_minutes(u).is_some())))
        })
    };
    match word(0) {
        Some("every") => match word(1) {
            Some(
                "day" | "days" | "weekday" | "weekdays" | "weekend" | "weekends" | "hour"
                | "minute",
            ) => true,
            Some(w) if w.starts_with(|c: char| c.is_ascii_digit()) => duration(1),
            Some(w) => {
                w.split(',')
                    .filter(|p| !p.is_empty())
                    .all(|p| day_span(p).is_some())
                    && !w.trim_matches(',').is_empty()
            }
            None => false,
        },
        Some("daily") => time(word(1)),
        Some("at") => time(word(1)),
        Some("tomorrow") => time(word(1)) || (word(1) == Some("at") && time(word(2))),
        Some("tonight") => head <= 2 && ws.iter().take(head).any(|w| w.colon),
        Some("in") => duration(1),
        _ => false,
    }
}

// ---- protocols ------------------------------------------------------------------------------------------------------

/// A saved protocol, as sphered keeps it and the clients show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protocol {
    /// `p1`, `p2`, ...
    pub id: String,
    /// The line as typed.
    pub text: String,
    pub schedule: Schedule,
    /// The folder of the repo it works in.
    pub repo: String,
    pub prompt: String,
    pub enabled: bool,
    /// Unix seconds.
    pub created_at: i64,
    #[serde(default)]
    pub last_run: Option<LastRun>,
    /// Unix seconds. None while it is off, and for a one-shot that has run.
    #[serde(default)]
    pub next_run: Option<i64>,
}

/// The latest time a protocol came due.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastRun {
    /// Unix seconds: when sphered acted on it.
    pub at: i64,
    /// The task it started. None when the run was left out or could not start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Why the run was left out: `still running`, `daily cap`, `missed while off`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    /// Why the task could not start, such as a repo that is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// How a protocol's last run went, for showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It has not come due yet.
    Never,
    /// The task it started is queued or working.
    Running,
    Done,
    NeedsYou,
    Failed,
    Stopped,
    /// The run was left out, and why.
    Skipped(String),
    /// The task could not start, and why.
    Error(String),
}

impl Outcome {
    /// `not run yet`, `running`, `done`, `needs you`, `failed`, `stopped`, `skipped: still running`.
    pub fn words(&self) -> String {
        match self {
            Outcome::Never => "not run yet".to_string(),
            Outcome::Running => "running".to_string(),
            Outcome::Done => "done".to_string(),
            Outcome::NeedsYou => "needs you".to_string(),
            Outcome::Failed => "failed".to_string(),
            Outcome::Stopped => "stopped".to_string(),
            Outcome::Skipped(why) => format!("skipped: {why}"),
            Outcome::Error(why) => format!("could not start: {why}"),
        }
    }
}

impl Protocol {
    /// How the last run went. `state_of` says how a task is doing now.
    pub fn outcome(&self, state_of: impl Fn(&str) -> Option<TaskState>) -> Outcome {
        let Some(last) = &self.last_run else {
            return Outcome::Never;
        };
        if let Some(why) = &last.skipped {
            return Outcome::Skipped(why.clone());
        }
        if let Some(why) = &last.error {
            return Outcome::Error(why.clone());
        }
        match last.task.as_deref().and_then(state_of) {
            Some(TaskState::Queued | TaskState::Working) => Outcome::Running,
            Some(TaskState::NeedsYou) => Outcome::NeedsYou,
            Some(TaskState::Done) => Outcome::Done,
            Some(TaskState::Failed) => Outcome::Failed,
            Some(TaskState::Stopped) => Outcome::Stopped,
            None => Outcome::Never,
        }
    }

    /// The name of the repo's folder.
    pub fn repo_name(&self) -> &str {
        self.repo
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&self.repo)
    }

    /// A short name for the protocol and the tasks it starts.
    pub fn name(&self) -> String {
        short_name(&self.prompt, 40)
    }
}

/// The first clause of a prompt as a title: `update the dependencies, run the tests` is `Update the dependencies`.
/// Cut at a word when longer than `max` characters.
pub fn short_name(prompt: &str, max: usize) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let clause = [", ", "; ", ". ", " and ", " then ", " - ", ": "]
        .iter()
        .filter_map(|sep| line.find(sep))
        .min()
        .map_or(line, |i| &line[..i]);
    let clause = clause.trim_end_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace());
    let mut name: String = clause.chars().take(max).collect();
    // Cut in the middle of a word: drop the part of it.
    if clause.chars().nth(max).is_some_and(|c| !c.is_whitespace()) {
        if let Some(i) = name.rfind(' ') {
            name.truncate(i);
        }
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Protocol".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-03-04 is a Wednesday.
    fn utc(y: i64, m: u32, d: u32, hour: i64, minute: i64) -> i64 {
        days_from_civil(y, m, d) * DAY + hour * 3600 + minute * 60
    }

    /// A zone with the United States' daylight saving in 2026: UTC-5, UTC-4 from March 8 07:00 UTC to November 1
    /// 06:00 UTC.
    struct Us;

    impl Zone for Us {
        fn offset(&self, utc: i64) -> i64 {
            let (spring, fall) = (self::utc(2026, 3, 8, 7, 0), self::utc(2026, 11, 1, 6, 0));
            if (spring..fall).contains(&utc) {
                -4 * 3600
            } else {
                -5 * 3600
            }
        }
    }

    const BERLIN: FixedZone = FixedZone(3600);

    fn ok(line: &str) -> Parsed {
        let now = utc(2026, 3, 4, 12, 0);
        parse(line, now, &BERLIN).unwrap_or_else(|e| panic!("{line}: {e}"))
    }

    fn bad(line: &str) -> String {
        parse(line, utc(2026, 3, 4, 12, 0), &BERLIN)
            .expect_err(line)
            .0
    }

    fn weekly(days: &[Weekday], hour: u8, minute: u8) -> Schedule {
        Schedule::Weekly {
            days: days.to_vec(),
            hour,
            minute,
        }
    }

    #[test]
    fn calendar_dates_and_days_convert_both_ways() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2026, 3, 4), 20_516);
        assert_eq!(civil_from_days(20_516), (2026, 3, 4));
        for z in (-800_000..800_000).step_by(997) {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        // 2026-03-04 was a Wednesday, 1970-01-01 a Thursday, 2000-02-29 a Tuesday.
        assert_eq!(Weekday::of_days(20_516), Weekday::Wed);
        assert_eq!(Weekday::of_days(0), Weekday::Thu);
        assert_eq!(Weekday::of_days(days_from_civil(2000, 2, 29)), Weekday::Tue);
    }

    #[test]
    fn the_line_in_the_brief_reads_as_written() {
        let p = ok(
            "every weekday 9:00 sphere: update the dependencies, run the tests and fix what breaks",
        );
        assert_eq!(p.schedule, weekly(&Weekday::WEEKDAYS, 9, 0));
        assert_eq!(p.repo.as_deref(), Some("sphere"));
        assert_eq!(
            p.prompt,
            "update the dependencies, run the tests and fix what breaks"
        );
    }

    #[test]
    fn every_kind_of_schedule_reads() {
        let all = Weekday::ALL;
        let cases: &[(&str, Schedule)] = &[
            ("every day 9:00", weekly(&all, 9, 0)),
            ("every day at 9:00", weekly(&all, 9, 0)),
            ("daily 9:00", weekly(&all, 9, 0)),
            ("daily at 9", weekly(&all, 9, 0)),
            ("every weekday 9:00", weekly(&Weekday::WEEKDAYS, 9, 0)),
            ("every weekend 10:30", weekly(&Weekday::WEEKEND, 10, 30)),
            ("every monday 9:00", weekly(&[Weekday::Mon], 9, 0)),
            ("every Mondays at 9am", weekly(&[Weekday::Mon], 9, 0)),
            (
                "every mon,wed,fri 18:30",
                weekly(&[Weekday::Mon, Weekday::Wed, Weekday::Fri], 18, 30),
            ),
            (
                "every fri, mon and wed 6:30pm",
                weekly(&[Weekday::Mon, Weekday::Wed, Weekday::Fri], 18, 30),
            ),
            ("every mon-fri 7:15", weekly(&Weekday::WEEKDAYS, 7, 15)),
            (
                "every sat-mon 7:15",
                weekly(&[Weekday::Mon, Weekday::Sat, Weekday::Sun], 7, 15),
            ),
            (
                "every hour",
                Schedule::Every {
                    minutes: 60,
                    from: utc(2026, 3, 4, 12, 0),
                },
            ),
            (
                "every 2 hours",
                Schedule::Every {
                    minutes: 120,
                    from: utc(2026, 3, 4, 12, 0),
                },
            ),
            (
                "every 30 minutes",
                Schedule::Every {
                    minutes: 30,
                    from: utc(2026, 3, 4, 12, 0),
                },
            ),
            (
                "every 45m",
                Schedule::Every {
                    minutes: 45,
                    from: utc(2026, 3, 4, 12, 0),
                },
            ),
            // Wednesday 12:00 UTC is 13:00 in Berlin.
            (
                "at 18:00",
                Schedule::Once {
                    at: utc(2026, 3, 4, 17, 0),
                },
            ),
            (
                "tomorrow 9:00",
                Schedule::Once {
                    at: utc(2026, 3, 5, 8, 0),
                },
            ),
            (
                "tomorrow at 9:00",
                Schedule::Once {
                    at: utc(2026, 3, 5, 8, 0),
                },
            ),
            (
                "tonight",
                Schedule::Once {
                    at: utc(2026, 3, 5, 1, 0),
                },
            ),
            (
                "in 2 hours",
                Schedule::Once {
                    at: utc(2026, 3, 4, 14, 0),
                },
            ),
            (
                "in 45 minutes",
                Schedule::Once {
                    at: utc(2026, 3, 4, 12, 45),
                },
            ),
            (
                "in 1 hour",
                Schedule::Once {
                    at: utc(2026, 3, 4, 13, 0),
                },
            ),
        ];
        for (schedule, want) in cases {
            for line in [format!("{schedule} sphere: go"), format!("{schedule}: go")] {
                let p = ok(&line);
                assert_eq!(&p.schedule, want, "{line}");
                assert_eq!(p.prompt, "go", "{line}");
            }
        }
    }

    #[test]
    fn times_read_in_every_form() {
        for (text, want) in [
            ("9", (9, 0)),
            ("9:00", (9, 0)),
            ("09:00", (9, 0)),
            ("9am", (9, 0)),
            ("6pm", (18, 0)),
            ("6:30pm", (18, 30)),
            ("18:30", (18, 30)),
            ("0:00", (0, 0)),
            ("23:59", (23, 59)),
            ("12am", (0, 0)),
            ("12pm", (12, 0)),
            ("12:15am", (0, 15)),
            ("12:15pm", (12, 15)),
        ] {
            assert_eq!(clock(text), Some(want), "{text}");
        }
        for text in [
            "24:00", "9:60", "9:5", "0pm", "13pm", "0am", "pm", "", "9:", ":30", "123", "9.30",
        ] {
            assert_eq!(clock(text), None, "{text:?}");
        }
        // Spaced out, and with the colon that starts the prompt stuck on.
        assert_eq!(
            ok("every day 6 pm sphere: x").schedule,
            weekly(&Weekday::ALL, 18, 0)
        );
        assert_eq!(
            ok("every day 9:00: x").schedule,
            weekly(&Weekday::ALL, 9, 0)
        );
        assert_eq!(ok("every day 9: x").schedule, weekly(&Weekday::ALL, 9, 0));
    }

    #[test]
    fn the_repo_is_optional_and_the_prompt_may_hold_colons() {
        let p = ok("every day 9:00: do it");
        assert_eq!((p.repo, p.prompt.as_str()), (None, "do it"));
        let p = ok("every day 9:00 ~/Projects/sphere: see https://example.com: now");
        assert_eq!(p.repo.as_deref(), Some("~/Projects/sphere"));
        assert_eq!(p.prompt, "see https://example.com: now");
        let p = ok("  EVERY Weekday 9:00   Sphere  :   Tidy up  ");
        assert_eq!(p.repo.as_deref(), Some("Sphere"));
        assert_eq!(p.prompt, "Tidy up");
    }

    #[test]
    fn what_is_wrong_is_said_in_words() {
        assert_eq!(
            bad("evry weekday 9:00 sphere: x"),
            "didn't understand 'evry': try 'every weekday 9:00 repo: what to do'"
        );
        assert!(bad("every weekday 25:00 sphere: x").contains("didn't understand the time '25:00'"));
        assert!(bad("every weekday sphere: x").starts_with("which time?"));
        assert!(bad("every weekday: x").starts_with("which time?"));
        assert!(bad("every banana 9:00 sphere: x").contains("didn't understand 'every banana'"));
        assert!(bad("every 9:00 sphere: x").contains("didn't understand 'every 9:00'"));
        assert!(bad("every sphere: x").contains("didn't understand 'every sphere'"));
        assert!(bad("every").starts_with("every what?"));
        assert!(
            bad("every 2 fortnights sphere: x").contains("didn't understand 'every 2 fortnights'")
        );
        assert!(bad("in 2 sphere: x").contains("didn't understand 'in 2 sphere'"));
        assert!(bad("in 2: x").starts_with("in 2 what?"));
        assert!(bad("in sphere: x").starts_with("in how long?"));
        assert!(bad("in 0 minutes: x").starts_with("in how long?"));
        assert!(bad("tomorrow sphere: x").starts_with("which time?"));
        assert!(bad("every day 9:00 sphere").starts_with("what should it do?"));
        assert!(bad("every day 9:00 sphere:").starts_with("what should it do?"));
        assert!(bad("every day 9:00 sphere:   ").starts_with("what should it do?"));
        assert!(
            bad("every day 9:00 fix the build: x").contains("didn't understand 'fix the build'")
        );
        assert!(bad("").starts_with("write when it runs"));
        assert!(bad("   ").starts_with("write when it runs"));
        assert!(bad("sphere: do things").contains("didn't understand 'sphere'"));
    }

    #[test]
    fn a_run_closer_than_fifteen_minutes_is_too_often() {
        assert_eq!(
            bad("every 10 minutes sphere: x"),
            "every 10 minutes is too often: the minimum is every 15 minutes"
        );
        assert_eq!(
            bad("every minute sphere: x"),
            "every minute is too often: the minimum is every 15 minutes"
        );
        assert_eq!(
            bad("every 1 minute sphere: x"),
            "every minute is too often: the minimum is every 15 minutes"
        );
        assert_eq!(
            ok("every 15 minutes sphere: x").schedule,
            Schedule::Every {
                minutes: 15,
                from: utc(2026, 3, 4, 12, 0)
            }
        );
        assert!(bad("every 400 hours sphere: x").contains("more than a week"));
        assert!(bad("every 99999999999 hours sphere: x")
            .contains("didn't understand 'every 99999999999 hours'"));
    }

    #[test]
    fn at_a_time_that_has_passed_means_tomorrow() {
        // 13:00 in Berlin.
        let later = ok("at 13:01: x").schedule;
        assert_eq!(
            later,
            Schedule::Once {
                at: utc(2026, 3, 4, 12, 1)
            }
        );
        let same = ok("at 13:00: x").schedule;
        assert_eq!(
            same,
            Schedule::Once {
                at: utc(2026, 3, 5, 12, 0)
            }
        );
        let earlier = ok("at 9: x").schedule;
        assert_eq!(
            earlier,
            Schedule::Once {
                at: utc(2026, 3, 5, 8, 0)
            }
        );
    }

    #[test]
    fn tonight_is_the_next_two_in_the_morning() {
        // At 13:00 that is 02:00 tomorrow; at 01:00 it is an hour away.
        let at = |h: i64, m: i64| parse("tonight: x", utc(2026, 3, 4, h, m), &BERLIN);
        assert_eq!(
            at(12, 0).unwrap().schedule,
            Schedule::Once {
                at: utc(2026, 3, 5, 1, 0)
            }
        );
        assert_eq!(
            at(0, 0).unwrap().schedule,
            Schedule::Once {
                at: utc(2026, 3, 4, 1, 0)
            }
        );
        assert_eq!(
            at(1, 30).unwrap().schedule,
            Schedule::Once {
                at: utc(2026, 3, 5, 1, 0)
            }
        );
    }

    #[test]
    fn weekly_runs_skip_the_days_not_named() {
        let s = weekly(&Weekday::WEEKDAYS, 9, 0);
        // Friday 2026-03-06 10:00 UTC is 11:00 in Berlin: next is Monday 09:00 Berlin = 08:00 UTC.
        assert_eq!(
            s.next_after(utc(2026, 3, 6, 10, 0), &BERLIN),
            Some(utc(2026, 3, 9, 8, 0))
        );
        // Before 09:00 on a weekday it is the same day; at 09:00 sharp, the next day.
        assert_eq!(
            s.next_after(utc(2026, 3, 4, 7, 59), &BERLIN),
            Some(utc(2026, 3, 4, 8, 0))
        );
        assert_eq!(
            s.next_after(utc(2026, 3, 4, 8, 0), &BERLIN),
            Some(utc(2026, 3, 5, 8, 0))
        );
        let one = weekly(&[Weekday::Wed], 9, 0);
        assert_eq!(
            one.next_after(utc(2026, 3, 4, 8, 0), &BERLIN),
            Some(utc(2026, 3, 11, 8, 0))
        );
        // The local day is not the UTC one: Wednesday 23:30 UTC is already Thursday in Berlin.
        let thu = weekly(&[Weekday::Thu], 0, 15);
        assert_eq!(
            thu.next_after(utc(2026, 3, 4, 23, 30), &BERLIN),
            Some(utc(2026, 3, 11, 23, 15))
        );
        assert_eq!(weekly(&[], 9, 0).next_after(0, &BERLIN), None);
    }

    #[test]
    fn a_repeating_protocol_keeps_its_beat() {
        let from = utc(2026, 3, 4, 12, 7);
        let s = Schedule::Every { minutes: 30, from };
        let min = |m: i64| from + m * 60;
        // The first run is one gap after it was written, however soon one asks.
        assert_eq!(s.next_after(from - 3600, &BERLIN), Some(min(30)));
        assert_eq!(s.next_after(from, &BERLIN), Some(min(30)));
        assert_eq!(s.next_after(min(29), &BERLIN), Some(min(30)));
        assert_eq!(s.next_after(min(30), &BERLIN), Some(min(60)));
        // A run that started late, or a computer that woke late, doesn't move the beat.
        assert_eq!(s.next_after(min(30) + 20, &BERLIN), Some(min(60)));
        assert_eq!(s.next_after(min(300) + 1, &BERLIN), Some(min(330)));
    }

    #[test]
    fn a_one_shot_runs_once() {
        let s = Schedule::Once { at: 1000 };
        assert_eq!(s.next_after(999, &BERLIN), Some(1000));
        assert_eq!(s.next_after(1000, &BERLIN), None);
        assert!(s.is_one_shot() && !weekly(&Weekday::ALL, 9, 0).is_one_shot());
    }

    #[test]
    fn the_same_wall_clock_time_follows_daylight_saving() {
        let s = weekly(&Weekday::ALL, 9, 0);
        // 09:00 is 14:00 UTC in winter and 13:00 UTC once the clocks have gone forward on March 8.
        let t = s.next_after(utc(2026, 3, 7, 15, 0), &Us).unwrap();
        assert_eq!(t, utc(2026, 3, 8, 13, 0));
        let t = s.next_after(utc(2026, 3, 6, 15, 0), &Us).unwrap();
        assert_eq!(t, utc(2026, 3, 7, 14, 0));
        // And back on November 1: 09:00 is 13:00 UTC before it and 14:00 UTC after.
        assert_eq!(
            s.next_after(utc(2026, 10, 31, 14, 0), &Us),
            Some(utc(2026, 11, 1, 14, 0))
        );
        assert_eq!(
            s.next_after(utc(2026, 10, 30, 14, 0), &Us),
            Some(utc(2026, 10, 31, 13, 0))
        );
    }

    #[test]
    fn a_time_the_clocks_skip_runs_just_after_the_gap() {
        // On March 8 02:00 jumps to 03:00 (07:00 UTC): 02:30 does not exist and 03:30 (07:30 UTC) stands in.
        let s = weekly(&Weekday::ALL, 2, 30);
        assert_eq!(
            s.next_after(utc(2026, 3, 8, 6, 0), &Us),
            Some(utc(2026, 3, 8, 7, 30))
        );
        // The day before and after it is 02:30 as usual.
        assert_eq!(
            s.next_after(utc(2026, 3, 7, 0, 0), &Us),
            Some(utc(2026, 3, 7, 7, 30))
        );
        assert_eq!(
            s.next_after(utc(2026, 3, 8, 8, 0), &Us),
            Some(utc(2026, 3, 9, 6, 30))
        );
    }

    #[test]
    fn a_time_the_clocks_pass_twice_runs_the_first_time() {
        // On November 1 02:00 falls back to 01:00 (06:00 UTC): 01:30 happens at 05:30 and again at 06:30 UTC.
        let s = weekly(&Weekday::ALL, 1, 30);
        assert_eq!(
            s.next_after(utc(2026, 11, 1, 0, 0), &Us),
            Some(utc(2026, 11, 1, 5, 30))
        );
        // Once that has run, the next is the day after, not the repeat.
        assert_eq!(
            s.next_after(utc(2026, 11, 1, 5, 30), &Us),
            Some(utc(2026, 11, 2, 6, 30))
        );
    }

    #[test]
    fn local_times_map_to_one_moment() {
        for z in [&BERLIN as &dyn Zone, &Us, &FixedZone(-9 * 3600 - 1800)] {
            for t in (utc(2026, 1, 1, 0, 0)..utc(2027, 1, 1, 0, 0)).step_by(7919) {
                let local = t + z.offset(t);
                let back = z.to_utc(local);
                // Wherever the reading means one moment, it is that moment; in the hour clocks repeat it is one of two.
                assert_eq!(z.offset(back) + back, local, "t={t}");
                assert!(back == t || (back - t).abs() == 3600, "t={t} back={back}");
            }
        }
    }

    #[test]
    fn the_local_zone_agrees_with_itself() {
        let now = utc(2026, 7, 1, 12, 0);
        let off = LocalZone.offset(now);
        assert!(off.abs() <= 14 * 3600);
        let local = now + off;
        assert_eq!(LocalZone.to_utc(local), now);
    }

    #[test]
    fn schedules_are_described_the_way_they_were_written() {
        let now = utc(2026, 3, 4, 12, 0);
        let d = |line: &str| ok(line).schedule.describe(now, &BERLIN);
        assert_eq!(d("every weekday 9:00: x"), "every weekday 09:00");
        assert_eq!(d("daily 9am: x"), "every day 09:00");
        assert_eq!(d("every monday 18:30: x"), "every monday 18:30");
        assert_eq!(d("every sat,sun 8:00: x"), "every weekend 08:00");
        assert_eq!(d("every mon,wed,fri 18:30: x"), "every mon, wed, fri 18:30");
        assert_eq!(d("every hour: x"), "every hour");
        assert_eq!(d("every 2 hours: x"), "every 2 hours");
        assert_eq!(d("every 90 minutes: x"), "every 90 minutes");
        assert_eq!(d("every 15 min: x"), "every 15 minutes");
        // 18:00 in Berlin is today, tomorrow's 9:00 is named by its weekday.
        assert_eq!(d("at 18:00: x"), "once · 18:00");
        assert_eq!(d("tomorrow 9:00: x"), "once · Thu 09:00");
    }

    #[test]
    fn moments_are_put_as_briefly_as_they_can_be() {
        let now = utc(2026, 3, 4, 12, 0); // Wednesday, 13:00 in Berlin
        let w = |t: i64| when_words(t, now, &BERLIN);
        assert_eq!(w(utc(2026, 3, 4, 17, 0)), "18:00");
        assert_eq!(w(utc(2026, 3, 4, 23, 30)), "Thu 00:30");
        assert_eq!(w(utc(2026, 3, 5, 8, 0)), "Thu 09:00");
        assert_eq!(w(utc(2026, 3, 9, 8, 0)), "Mon 09:00");
        assert_eq!(w(utc(2026, 3, 11, 8, 0)), "Wed 11 Mar 09:00");
        assert_eq!(w(utc(2026, 12, 24, 8, 0)), "Thu 24 Dec 09:00");
        assert_eq!(w(utc(2027, 1, 4, 8, 0)), "Mon 4 Jan 2027 09:00");
        // Earlier today and earlier days read the same way.
        assert_eq!(w(utc(2026, 3, 4, 7, 15)), "08:15");
        assert_eq!(w(utc(2026, 3, 3, 8, 0)), "Tue 3 Mar 09:00");
    }

    #[test]
    fn spans_are_put_in_the_biggest_unit_that_fits() {
        assert_eq!(span_words(-5), "<1 min");
        assert_eq!(span_words(59), "<1 min");
        assert_eq!(span_words(60), "1 min");
        assert_eq!(span_words(59 * 60 + 59), "59 min");
        assert_eq!(span_words(3600), "1 h");
        assert_eq!(span_words(3 * 3600 + 1799), "3 h");
        assert_eq!(span_words(DAY - 1), "23 h");
        assert_eq!(span_words(2 * DAY + 5), "2 d");
    }

    #[test]
    fn a_task_that_only_starts_like_a_schedule_stays_a_task() {
        let now = utc(2026, 3, 4, 12, 0);
        let kind = |line: &str| classify(line, now, &BERLIN);
        for line in [
            "sphere: update the dependencies",
            "fix the failing test",
            "every test in the repo needs a comment: add them",
            "every file under src should have a header",
            "every function in parser.rs is too long: split them",
            "at the end of the run, print a summary",
            "at 5 places we use the old API: replace them",
            "in the parser, handle the empty case",
            "in 2 places the cache is stale: fix them",
            "tonight we should refactor the parser: plan it",
            "tomorrow is the release: write the notes",
            "daily standup notes: write them",
            "daily",
            "every",
            "tonight we",
            "",
        ] {
            assert_eq!(kind(line), Line::Task, "{line}");
        }
    }

    #[test]
    fn a_line_that_starts_with_a_schedule_is_a_protocol_or_a_mistake() {
        let now = utc(2026, 3, 4, 12, 0);
        let kind = |line: &str| classify(line, now, &BERLIN);
        for line in [
            "every weekday 9:00 sphere: update the dependencies",
            "every day 9:00: run the tests",
            "daily 9:00 sphere: x",
            "at 18:00 sphere: x",
            "at 9 sphere: x",
            "tomorrow 9:00 sphere: x",
            "tomorrow 9 sphere: x",
            "tonight sphere: x",
            "tonight: x",
            "in 2 hours sphere: x",
            "in 45 minutes: x",
            "every hour sphere: x",
            "every 2 hours sphere: x",
            "every mon,wed 18:30 sphere: x",
        ] {
            assert!(matches!(kind(line), Line::Protocol(_)), "{line}");
        }
        for line in [
            "every weekday 25:00 sphere: x",
            "every weekday sphere: x",
            "every monday sphere: x",
            "every 10 minutes sphere: x",
            "every minute: x",
            "every 2 hours",
            "every day 9:00 fix the build: x",
            "at 18:00 x",
            "at 25:00 sphere: x",
            "daily 9:00",
            "in 2 hours",
            "tomorrow 9:00 sphere",
        ] {
            assert!(matches!(kind(line), Line::Mistake(_)), "{line}");
        }
    }

    fn protocol(last_run: Option<LastRun>) -> Protocol {
        Protocol {
            id: "p1".into(),
            text: "every day 9:00 sphere: x".into(),
            schedule: weekly(&Weekday::ALL, 9, 0),
            repo: "/home/me/Projects/sphere".into(),
            prompt: "update the dependencies, run the tests and fix what breaks".into(),
            enabled: true,
            created_at: 0,
            last_run,
            next_run: Some(100),
        }
    }

    #[test]
    fn the_last_run_is_what_its_task_came_to() {
        let ran = |task: &str| LastRun {
            at: 5,
            task: Some(task.into()),
            skipped: None,
            error: None,
        };
        let state = |t: &str| match t {
            "t1" => Some(TaskState::Done),
            "t2" => Some(TaskState::NeedsYou),
            "t3" => Some(TaskState::Failed),
            "t4" => Some(TaskState::Working),
            "t5" => Some(TaskState::Queued),
            "t6" => Some(TaskState::Stopped),
            _ => None,
        };
        let outcome = |last: Option<LastRun>| protocol(last).outcome(state);
        assert_eq!(outcome(None), Outcome::Never);
        assert_eq!(outcome(Some(ran("t1"))), Outcome::Done);
        assert_eq!(outcome(Some(ran("t2"))), Outcome::NeedsYou);
        assert_eq!(outcome(Some(ran("t3"))), Outcome::Failed);
        assert_eq!(outcome(Some(ran("t4"))), Outcome::Running);
        assert_eq!(outcome(Some(ran("t5"))), Outcome::Running);
        assert_eq!(outcome(Some(ran("t6"))), Outcome::Stopped);
        assert_eq!(outcome(Some(ran("t9"))), Outcome::Never);
        let skipped = LastRun {
            at: 5,
            task: None,
            skipped: Some("still running".into()),
            error: None,
        };
        assert_eq!(
            outcome(Some(skipped)),
            Outcome::Skipped("still running".into())
        );
        let error = LastRun {
            at: 5,
            task: None,
            skipped: None,
            error: Some("no git repo".into()),
        };
        assert_eq!(outcome(Some(error)), Outcome::Error("no git repo".into()));
    }

    #[test]
    fn outcomes_have_words() {
        assert_eq!(Outcome::Never.words(), "not run yet");
        assert_eq!(Outcome::NeedsYou.words(), "needs you");
        assert_eq!(
            Outcome::Skipped("daily cap".into()).words(),
            "skipped: daily cap"
        );
        assert_eq!(
            Outcome::Error("no git repo".into()).words(),
            "could not start: no git repo"
        );
    }

    #[test]
    fn a_protocol_is_named_after_its_prompt_and_repo() {
        let p = protocol(None);
        assert_eq!(p.repo_name(), "sphere");
        assert_eq!(p.name(), "Update the dependencies");
        let mut q = p.clone();
        q.repo = "/home/me/Projects/sphere/".into();
        assert_eq!(q.repo_name(), "sphere");
        q.repo = "sphere".into();
        assert_eq!(q.repo_name(), "sphere");
    }

    #[test]
    fn short_names_are_the_first_clause_cut_at_a_word() {
        let n = |p: &str, max: usize| short_name(p, max);
        assert_eq!(n("fix the flaky test", 40), "Fix the flaky test");
        assert_eq!(n("fix the flaky test.", 40), "Fix the flaky test");
        assert_eq!(n("update deps; run the tests", 40), "Update deps");
        assert_eq!(n("run the tests and fix what breaks", 40), "Run the tests");
        assert_eq!(
            n("\n\n  triage the inbox\nthen reply", 40),
            "Triage the inbox"
        );
        assert_eq!(
            n("summarise every open pull request for the whole team", 20),
            "Summarise every open"
        );
        assert_eq!(n("", 40), "Protocol");
        assert_eq!(n("...", 40), "Protocol");
        assert_eq!(n("éclair the cache", 40), "Éclair the cache");
        assert_eq!(n("supercalifragilisticexpialidocious", 10), "Supercalif");
    }

    #[test]
    fn a_protocol_survives_the_wire() {
        let mut p = protocol(Some(LastRun {
            at: 7,
            task: Some("t3".into()),
            skipped: None,
            error: None,
        }));
        p.schedule = Schedule::Every {
            minutes: 30,
            from: 12,
        };
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["schedule"]["type"], "every");
        assert_eq!(json["last_run"], serde_json::json!({"at": 7, "task": "t3"}));
        let back: Protocol = serde_json::from_value(json).unwrap();
        assert_eq!(back, p);
        let mon_fri = serde_json::to_value(weekly(&[Weekday::Mon, Weekday::Fri], 9, 5)).unwrap();
        assert_eq!(
            mon_fri,
            serde_json::json!({"type": "weekly", "days": ["mon", "fri"], "hour": 9, "minute": 5})
        );
        // A protocol with no runs yet leaves them out.
        let bare = r#"{"id":"p1","text":"t","schedule":{"type":"once","at":9},"repo":"/r","prompt":"p","enabled":true,"created_at":1}"#;
        let p: Protocol = serde_json::from_str(bare).unwrap();
        assert!(p.last_run.is_none() && p.next_run.is_none());
    }
}
