//! The home's Next 24h rail: this Mac's Calendar events for the next day, through EventKit (macOS), read on a
//! background thread every five minutes. Access is asked for once; with no access, no calendar, or in screenshots
//! and demos (which never touch EventKit), the rail falls back to "Nothing scheduled". Event contents stay in
//! memory: nothing here logs them.
use bevy::prelude::*;
use std::sync::{Arc, Mutex};

/// How often the events are read again.
pub const REFRESH_SECS: u64 = 300;
/// How far ahead the rail looks.
pub const WINDOW_SECS: i64 = 24 * 3600;
/// Rows the rail has room for.
pub const SLOTS: usize = 4;
/// The longest title a row shows before it's clipped.
pub const TITLE_CHARS: usize = 26;

/// One calendar event, as the rail needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub title: String,
    /// unix seconds
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    /// the calendar's colour, sRGB 0..1
    pub color: Option<[f32; 3]>,
}

/// What the calendar thread knows.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Feed {
    /// no calendar here: not macOS, not the bundled app, or a screenshot or demo
    #[default]
    Off,
    /// waiting for the first answer (the access prompt may be up)
    Asking,
    /// access denied or restricted
    Denied,
    Events(Vec<Event>),
}

/// The shared feed and a counter that moves each time it changes.
#[derive(Resource, Clone, Default)]
pub struct Calendar(pub Arc<Mutex<(u64, Feed)>>);

impl Calendar {
    pub fn get(&self) -> (u64, Feed) {
        self.0.lock().map(|g| g.clone()).unwrap_or_default()
    }
    fn set(&self, f: Feed) {
        if let Ok(mut g) = self.0.lock() {
            if g.1 != f {
                g.0 += 1;
                g.1 = f;
            }
        }
    }
}

/// `real`: read this Mac's Calendar (the windowed app). Otherwise `demo` events, or none.
pub fn start(real: bool, demo: bool) -> Calendar {
    let cal = Calendar::default();
    if !real {
        if demo {
            cal.set(Feed::Events(demo_events(crate::core::unix_now())));
        }
        return cal;
    }
    #[cfg(target_os = "macos")]
    if ek::usable() {
        cal.set(Feed::Asking);
        let out = cal.clone();
        let _ = std::thread::Builder::new().name("sphere-calendar".into()).spawn(move || ek::run(&out));
    }
    cal
}

/// The look test's events (`--calendar`): a meeting on now, two later today, one tomorrow.
pub fn demo_events(now: i64) -> Vec<Event> {
    let h = 3600;
    vec![
        Event { title: "Design review".into(), start: now - h / 4, end: now + h / 2, all_day: false, color: Some([0.98, 0.42, 0.29]) },
        Event { title: "Ship calendar rail to the home screen".into(), start: now + 2 * h, end: now + 3 * h, all_day: false, color: Some([0.30, 0.65, 0.98]) },
        Event { title: "1:1".into(), start: now + 5 * h, end: now + 5 * h + h / 2, all_day: false, color: Some([0.55, 0.85, 0.35]) },
        Event { title: "Release day".into(), start: now + 20 * h, end: now + 22 * h, all_day: false, color: None },
    ]
}

/// One rail row: the time, the title, the calendar's colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub time: String,
    pub title: String,
    pub color: Option<[f32; 3]>,
}

/// What the rail shows: rows, or an empty state and a hint under it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rail {
    pub rows: Vec<Row>,
    pub empty: Option<&'static str>,
    pub hint: Option<&'static str>,
}

/// The fallback chain: calendar events in the next 24 h, else "Nothing scheduled" with a hint saying why. (Sphere
/// has no scheduled agent runs yet, so there's no middle step.) `offset` is the local time zone's offset from UTC
/// at a time, in seconds.
pub fn rail(feed: &Feed, now: i64, offset: impl Fn(i64) -> i64, slots: usize) -> Rail {
    let nothing = |hint| Rail { rows: vec![], empty: Some("NOTHING SCHEDULED"), hint: Some(hint) };
    let evs = match feed {
        Feed::Off => return nothing("No calendar connected"),
        Feed::Asking => return nothing("Checking Calendar"),
        Feed::Denied => return nothing("Allow Calendar in System Settings"),
        Feed::Events(e) => e,
    };
    let mut next: Vec<&Event> = evs.iter().filter(|e| e.end > now && e.start < now + WINDOW_SECS).collect();
    if next.is_empty() {
        return nothing("Your next 24 hours are clear");
    }
    // all-day and on-now first (they started earliest), then by start
    next.sort_by_key(|e| (e.start.max(if e.all_day { i64::MIN } else { now }), e.end));
    let more = next.len().saturating_sub(slots);
    let shown = if more > 0 { slots.saturating_sub(1) } else { next.len() };
    let mut rows: Vec<Row> = next[..shown].iter().map(|e| Row { time: when(e, now, &offset), title: clip(&e.title, TITLE_CHARS), color: swatch(e.color) }).collect();
    if more > 0 {
        rows.push(Row { time: String::new(), title: format!("+{} more", next.len() - shown), color: None });
    }
    Rail { rows, empty: None, hint: None }
}

/// The row's time: ALL DAY, NOW for one under way, HH:MM today, TMRW HH:MM tomorrow (24-hour, like the clock).
pub fn when(e: &Event, now: i64, offset: impl Fn(i64) -> i64) -> String {
    let day = |t: i64| (t + offset(t)).div_euclid(86400);
    let tomorrow = day(e.start) > day(now);
    if e.all_day {
        return if tomorrow { "TMRW".into() } else { "ALL DAY".into() };
    }
    if e.start <= now {
        return "NOW".into();
    }
    let m = (e.start + offset(e.start)).rem_euclid(86400) / 60;
    let hm = format!("{:02}:{:02}", m / 60, m % 60);
    if tomorrow {
        format!("TMRW {hm}")
    } else {
        hm
    }
}

/// A title on one line, clipped to `n` characters with an ellipsis; "(No title)" for a blank one.
pub fn clip(s: &str, n: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.is_empty() {
        return "(No title)".into();
    }
    crate::appframe::clip(&one, n)
}

/// A calendar colour the rail can draw: each component clamped to 0..1, none if any is not a number.
pub fn swatch(c: Option<[f32; 3]>) -> Option<[f32; 3]> {
    let c = c?;
    if c.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some(c.map(|v| v.clamp(0.0, 1.0)))
}

#[cfg(target_os = "macos")]
mod ek {
    use super::{Calendar, Event, Feed, REFRESH_SECS, WINDOW_SECS};
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObjectProtocol};
    use objc2::sel;
    use objc2_app_kit::NSColorSpace;
    use objc2_event_kit::{EKAuthorizationStatus, EKEntityType, EKEventStore};
    use objc2_foundation::{NSBundle, NSDate, NSError, NSString};
    use std::sync::mpsc;
    use std::time::Duration;

    /// EventKit needs the app's bundle to say why it wants Calendar (a bare binary asking is killed), so only the
    /// packaged app reads it.
    pub fn usable() -> bool {
        let b = NSBundle::mainBundle();
        b.bundleIdentifier().is_some() && b.objectForInfoDictionaryKey(&NSString::from_str("NSCalendarsFullAccessUsageDescription")).is_some()
    }

    /// Ask once, then read the next 24 h every five minutes.
    pub fn run(out: &Calendar) {
        let mut store = unsafe { EKEventStore::new() };
        if !granted(&store) {
            out.set(Feed::Denied);
            return;
        }
        // a store made before access was granted sees no calendars
        store = unsafe { EKEventStore::new() };
        loop {
            out.set(Feed::Events(read(&store)));
            std::thread::sleep(Duration::from_secs(REFRESH_SECS));
        }
    }

    fn granted(store: &EKEventStore) -> bool {
        let status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
        if status == EKAuthorizationStatus::FullAccess {
            return true;
        }
        if status != EKAuthorizationStatus::NotDetermined {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        let done = block2::RcBlock::new(move |ok: Bool, _err: *mut NSError| {
            let _ = tx.send(ok.as_bool());
        });
        // macOS 14 asks for full access by name; older systems with the entity type
        if store.respondsToSelector(sel!(requestFullAccessToEventsWithCompletion:)) {
            unsafe { store.requestFullAccessToEventsWithCompletion(&*done as *const _ as *mut _) };
        } else {
            #[allow(deprecated)]
            unsafe {
                store.requestAccessToEntityType_completion(EKEntityType::Event, &*done as *const _ as *mut _)
            };
        }
        rx.recv().unwrap_or(false)
    }

    fn read(store: &EKEventStore) -> Vec<Event> {
        let from = NSDate::now();
        let to = NSDate::dateWithTimeIntervalSinceNow(WINDOW_SECS as f64);
        let pred = unsafe { store.predicateForEventsWithStartDate_endDate_calendars(&from, &to, None) };
        let evs = unsafe { store.eventsMatchingPredicate(&pred) };
        let srgb = NSColorSpace::sRGBColorSpace();
        let mut v = Vec::with_capacity(evs.count());
        for i in 0..evs.count() {
            let e = evs.objectAtIndex(i);
            let color = unsafe { e.calendar() }.and_then(|c| unsafe { c.color() }.colorUsingColorSpace(&srgb)).map(|c| [c.redComponent() as f32, c.greenComponent() as f32, c.blueComponent() as f32]);
            v.push(Event { title: unsafe { e.title() }.to_string(), start: secs(&unsafe { e.startDate() }), end: secs(&unsafe { e.endDate() }), all_day: unsafe { e.isAllDay() }, color });
        }
        v
    }

    fn secs(d: &Retained<NSDate>) -> i64 {
        d.timeIntervalSince1970() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UTC: fn(i64) -> i64 = |_| 0;
    // 2026-10-19 09:00 UTC
    const NOW: i64 = 1_792_400_400;

    fn ev(title: &str, start: i64, end: i64) -> Event {
        Event { title: title.into(), start, end, all_day: false, color: Some([0.2, 0.4, 0.6]) }
    }

    #[test]
    fn times_read_as_now_today_and_tomorrow() {
        assert_eq!(NOW % 86400, 9 * 3600);
        assert_eq!(when(&ev("a", NOW - 600, NOW + 600), NOW, UTC), "NOW");
        assert_eq!(when(&ev("a", NOW + 90 * 60, NOW + 7200), NOW, UTC), "10:30");
        assert_eq!(when(&ev("a", NOW + 14 * 3600 + 59 * 60, NOW + 16 * 3600), NOW, UTC), "23:59");
        assert_eq!(when(&ev("a", NOW + 15 * 3600, NOW + 16 * 3600), NOW, UTC), "TMRW 00:00");
        assert_eq!(when(&ev("a", NOW + 23 * 3600 + 5 * 60, NOW + 24 * 3600), NOW, UTC), "TMRW 08:05");
        // the local time zone moves both the clock and the day line
        let plus2: fn(i64) -> i64 = |_| 2 * 3600;
        assert_eq!(when(&ev("a", NOW + 3600, NOW + 7200), NOW, plus2), "12:00");
        assert_eq!(when(&ev("a", NOW + 13 * 3600, NOW + 14 * 3600), NOW, plus2), "TMRW 00:00");
        // 23:00 the day before, west of UTC: an hour on is past midnight
        let minus10: fn(i64) -> i64 = |_| -10 * 3600;
        assert_eq!(when(&ev("a", NOW + 3600, NOW + 7200), NOW, minus10), "TMRW 00:00");
    }

    #[test]
    fn all_day_events_say_so() {
        let today = NOW - NOW % 86400;
        let mut e = ev("Holiday", today, today + 86400);
        e.all_day = true;
        assert_eq!(when(&e, NOW, UTC), "ALL DAY");
        e.start += 86400;
        e.end += 86400;
        assert_eq!(when(&e, NOW, UTC), "TMRW");
        // all-day sorts first, ahead of one on now
        let mut a = ev("Holiday", today, today + 86400);
        a.all_day = true;
        let r = rail(&Feed::Events(vec![ev("Standup", NOW - 60, NOW + 600), a]), NOW, UTC, SLOTS);
        assert_eq!(r.rows.iter().map(|r| r.time.as_str()).collect::<Vec<_>>(), ["ALL DAY", "NOW"]);
    }

    #[test]
    fn titles_are_clipped_to_one_line() {
        assert_eq!(clip("Standup", 26), "Standup");
        assert_eq!(clip("  Plan\n the   week ", 26), "Plan the week");
        assert_eq!(clip("", 26), "(No title)");
        assert_eq!(clip(" \n ", 26), "(No title)");
        let long = clip("Quarterly planning with the whole platform team", 26);
        assert_eq!(long.chars().count(), 26);
        assert!(long.ends_with('…'));
        assert_eq!(clip("Ünïcödé 会議 with a long tail on it, really", 10), "Ünïcödé 会…");
    }

    #[test]
    fn colors_are_clamped_or_dropped() {
        assert_eq!(swatch(Some([0.2, 0.4, 0.6])), Some([0.2, 0.4, 0.6]));
        assert_eq!(swatch(Some([-0.1, 1.2, 0.5])), Some([0.0, 1.0, 0.5]));
        assert_eq!(swatch(Some([f32::NAN, 0.0, 0.0])), None);
        assert_eq!(swatch(None), None);
        let r = rail(&Feed::Events(vec![Event { color: Some([2.0, 0.5, f32::INFINITY]), ..ev("a", NOW + 60, NOW + 120) }]), NOW, UTC, SLOTS);
        assert_eq!(r.rows[0].color, None);
    }

    #[test]
    fn the_rail_shows_the_next_24_hours_in_order() {
        let evs = vec![
            ev("Later", NOW + 3 * 3600, NOW + 4 * 3600),
            ev("Over", NOW - 7200, NOW - 3600),
            ev("Too far", NOW + 25 * 3600, NOW + 26 * 3600),
            ev("Soon", NOW + 600, NOW + 1200),
            ev("On now", NOW - 600, NOW + 600),
        ];
        let r = rail(&Feed::Events(evs), NOW, UTC, SLOTS);
        assert_eq!(r.empty, None);
        assert_eq!(r.rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["On now", "Soon", "Later"]);
        assert_eq!(r.rows[1], Row { time: "09:10".into(), title: "Soon".into(), color: Some([0.2, 0.4, 0.6]) });
    }

    #[test]
    fn overflow_folds_into_a_more_row() {
        let evs: Vec<Event> = (1..=6).map(|i| ev(&format!("E{i}"), NOW + i * 600, NOW + i * 600 + 300)).collect();
        let r = rail(&Feed::Events(evs.clone()), NOW, UTC, 4);
        assert_eq!(r.rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["E1", "E2", "E3", "+3 more"]);
        assert_eq!(r.rows[3].time, "");
        // exactly enough room: no more row
        let r = rail(&Feed::Events(evs[..4].to_vec()), NOW, UTC, 4);
        assert_eq!(r.rows.len(), 4);
        assert!(r.rows.iter().all(|r| !r.title.starts_with('+')));
    }

    #[test]
    fn the_fallback_chain_ends_in_nothing_scheduled() {
        for (feed, hint) in [
            (Feed::Off, "No calendar connected"),
            (Feed::Asking, "Checking Calendar"),
            (Feed::Denied, "Allow Calendar in System Settings"),
            (Feed::Events(vec![]), "Your next 24 hours are clear"),
            (Feed::Events(vec![ev("Yesterday", NOW - 86400, NOW - 80000)]), "Your next 24 hours are clear"),
        ] {
            let r = rail(&feed, NOW, UTC, SLOTS);
            assert!(r.rows.is_empty(), "{feed:?}");
            assert_eq!(r.empty, Some("NOTHING SCHEDULED"));
            assert_eq!(r.hint, Some(hint));
        }
    }

    #[test]
    fn screenshots_and_demos_never_read_the_calendar() {
        // not real: the feed is set right away and no thread is started (the access prompt never shows)
        assert_eq!(start(false, false).get().1, Feed::Off);
        let (_, f) = start(false, true).get();
        let Feed::Events(e) = f else { panic!("demo events") };
        assert_eq!(e.len(), 4);
    }
}
