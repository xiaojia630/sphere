//! Hold the Swarm's rules, with no Bevy in them. Agents join a shift on a timetable, work, and ask before each step;
//! you answer from a queue (yes, change, always, allow all, later) before their patience runs out. Letting a
//! dangerous ask through, or leaving an agent waiting too long, costs one of three shields; the third ends the run.
//! A task whose steps all went through is merged.
//!
//! Everything random comes from the UTC date, so everyone plays the same run that day: each agent's asks come from
//! its own stream, so they are the same whatever you answer. The numbers and streams are the prototype's, which was
//! tuned by bots that play like an expert, a casual player, a greedy one (Always on everything) and a reckless one.
use super::content::{self, rule_for, rule_match, Tool, DANGER, JOIN_AT, ROSTER, SAFE};

// ---------------------------------------------------------------- the day

/// Days from 1970-01-01 to a date (proleptic Gregorian, Howard Hinnant's algorithm).
pub const fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day number from 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The first run.
const LAUNCH: i64 = days_from_civil(2026, 10, 20);
const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];

/// Today's run: the UTC date names it and seeds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Day {
    /// days since 1970-01-01
    pub n: i64,
    /// `2026-10-20`
    pub key: String,
    /// 1 on the first day
    pub run: u32,
    /// `20 OCT`
    pub label: String,
}

impl Day {
    pub fn of(n: i64) -> Day {
        let (y, m, d) = civil_from_days(n);
        Day { n, key: format!("{y:04}-{m:02}-{d:02}"), run: (n - LAUNCH + 1).max(1) as u32, label: format!("{d} {}", MONTHS[(m - 1) as usize]) }
    }
    pub fn at(unix: i64) -> Day {
        Day::of(unix.div_euclid(86_400))
    }
    /// The first run's day (screenshots and tests).
    pub fn first() -> Day {
        Day::of(LAUNCH)
    }
    /// `Run 001`
    pub fn run_label(&self) -> String {
        format!("Run {:03}", self.run)
    }
}

impl Default for Day {
    fn default() -> Self {
        Day::first()
    }
}

// ---------------------------------------------------------------- randomness

/// FNV-1a over the UTF-16 units (the prototype hashes a JavaScript string).
fn seed_of(s: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for c in s.encode_utf16() {
        h ^= u32::from(c);
        h = h.wrapping_mul(16_777_619);
    }
    h
}

/// The prototype's generator (mulberry32), seeded from the day and a name: `Rng::new("2026-10-20", "agent3")`.
#[derive(Clone, Debug)]
pub struct Rng(u32);

impl Rng {
    pub fn new(day: &str, key: &str) -> Rng {
        Rng(seed_of(&format!("{day}:{key}")))
    }
    /// 0..1
    pub fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6D2B_79F5);
        let a = self.0;
        let mut t = (a ^ (a >> 15)).wrapping_mul(1 | a);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[((self.next() * xs.len() as f64).floor() as usize).min(xs.len() - 1)]
    }
}

// ---------------------------------------------------------------- the run

/// A request in the queue.
#[derive(Clone, Debug, PartialEq)]
pub struct Req {
    pub id: u32,
    /// index into `Run::agents`
    pub agent: usize,
    pub tool: Tool,
    pub text: String,
    /// under the command: a diff's size, what a fetch is for
    pub detail: &'static str,
    pub danger: bool,
    /// why a dangerous one is
    pub why: &'static str,
    /// the Always rule it offers
    pub rule: Option<String>,
    /// why a safe one that looks scary is fine
    pub note: &'static str,
    pub born: f64,
    pub patience: f64,
    /// seconds of patience left
    pub left: f64,
    /// how many agents asked this at once (0: not a wave)
    pub wave: usize,
    /// the Always rule that answered it
    pub auto: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AState {
    Work,
    /// a request is up (or an Always rule is answering it)
    Wait,
    /// it waited too long and lost its turn
    Stall,
}

#[derive(Clone, Debug)]
pub struct Agent {
    /// its place in the roster (0..9)
    pub idx: usize,
    pub repo: &'static str,
    pub stack: &'static str,
    /// timing
    rng: Rng,
    /// what it asks
    ask: Rng,
    /// asks made
    pub k: u32,
    pub task_idx: usize,
    pub task: &'static str,
    /// tasks merged so far (each task is its own satellite)
    pub merged: u32,
    pub steps: u32,
    pub done: u32,
    pub state: AState,
    timer: f64,
    pub req: Option<u32>,
    stall_t: f64,
    /// the last thing it did, for its label
    pub last: String,
}

/// How a request was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Yes,
    Change,
    Always,
    /// Shift+Enter, for every agent asking the same
    All,
    /// an Always rule answered it before you saw it
    Auto,
}

/// What you can press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    Change,
    Always,
    All,
    Later,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Incident,
    Stall,
}

/// One answer on the results' row of marks: a yes, a catch, an incident, a slip (a needless block or a stall).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Yes,
    Caught,
    Incident,
    Slip,
}

impl Mark {
    pub fn emoji(self) -> &'static str {
        match self {
            Mark::Yes => "🟩",
            Mark::Caught => "🟦",
            Mark::Incident => "🟥",
            Mark::Slip => "🟨",
        }
    }
}

/// What happened, for the sounds, the flashes and the words on screen (drained each frame).
#[derive(Clone, Debug, PartialEq)]
pub enum Ev {
    Joined(usize),
    /// a request went into the queue
    Asked(usize),
    Wave { n: usize, trap: bool },
    Approved { agent: usize, pts: u32, how: How, text: String, rule: Option<String> },
    /// a task's steps all went through: merged (the task that finished)
    Merged { agent: usize, pts: u32, task: &'static str },
    Caught { agent: usize, pts: u32, text: String, why: &'static str },
    /// a safe ask blocked
    Needless { agent: usize, text: String, note: &'static str },
    /// a dangerous ask let through
    Incident { agent: usize, text: String, why: &'static str, how: How, many: usize, rule: Option<String> },
    Stalled { agent: usize, text: String },
    /// an Always rule kept
    Always(String),
    AllowedAll(usize),
    Later,
    /// a key that does nothing here (Always with no rule, Later with one ask)
    Nudge,
    Over(End),
}

pub const SHIELDS: u32 = 3;
/// An Always rule answers this long after the ask.
const AUTO_SECS: f64 = 0.26;
const STALL_SECS: f64 = 2.2;

#[derive(Clone, Debug)]
pub struct Run {
    pub day: String,
    pub t: f64,
    pub score: u32,
    pub shipped: u32,
    pub caught: u32,
    pub incidents: u32,
    pub stalls: u32,
    pub needless: u32,
    pub combo: u32,
    pub max_combo: u32,
    pub shields: u32,
    /// seconds agents waited on answers that came
    pub waited: f64,
    pub agents: Vec<Agent>,
    pub queue: Vec<Req>,
    autos: Vec<Req>,
    /// the request on the card
    pub cur: usize,
    pub rules: Vec<String>,
    pub marks: Vec<Mark>,
    seq: u32,
    next_wave: f64,
    wave_n: u32,
    joined: usize,
    pub end: Option<End>,
    /// what the last incident teaches
    pub lesson: String,
    pub events: Vec<Ev>,
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

impl Run {
    pub fn new(day: &str) -> Run {
        let mut r = Run {
            day: day.to_string(),
            t: 0.0,
            score: 0,
            shipped: 0,
            caught: 0,
            incidents: 0,
            stalls: 0,
            needless: 0,
            combo: 0,
            max_combo: 0,
            shields: SHIELDS,
            waited: 0.0,
            agents: Vec::new(),
            queue: Vec::new(),
            autos: Vec::new(),
            cur: 0,
            rules: Vec::new(),
            marks: Vec::new(),
            seq: 0,
            next_wave: 30.0,
            wave_n: 0,
            joined: 0,
            end: None,
            lesson: String::new(),
            events: Vec::new(),
        };
        for (i, at) in JOIN_AT.iter().enumerate() {
            if *at == 0.0 {
                r.add_agent(i);
                r.joined = i + 1;
            }
        }
        r.events.clear();
        r
    }

    pub fn playing(&self) -> bool {
        self.end.is_none()
    }

    /// The combo multiplier: one more for every five answers in a row, up to four.
    pub fn mult(&self) -> u32 {
        1 + (self.combo / 5).min(3)
    }

    /// How long a new ask waits for you: 10 s at first, down to 4.6 s.
    pub fn patience(&self) -> f64 {
        (10.0 - self.t * 0.026).max(4.6)
    }

    /// Seconds agents have waited, counting the asks still up.
    pub fn live_waited(&self) -> f64 {
        self.waited + self.queue.iter().map(|q| self.t - q.born).sum::<f64>()
    }

    /// The request on the card.
    pub fn current(&self) -> Option<&Req> {
        self.queue.get(self.cur)
    }

    /// How many requests in the queue ask the same as `req` (itself included).
    pub fn same_count(&self, req: &Req) -> usize {
        self.queue.iter().filter(|q| q.text == req.text && q.tool == req.tool).count()
    }

    fn work_time(&mut self, a: usize) -> f64 {
        (2.6 + self.agents[a].rng.next() * 3.6) * (1.0 - self.t / 320.0).max(0.5)
    }

    fn add_agent(&mut self, i: usize) {
        let (repo, task_idx) = ROSTER[i];
        let r = content::repo(repo);
        let mut rng = Rng::new(&self.day, &format!("agent{i}"));
        // the prototype drew its orbit phase and speed first: keep the stream where it was
        rng.next();
        rng.next();
        let steps = 3 + (rng.next() * 3.0).floor() as u32;
        let timer = 1.2 + i as f64 * 0.35 + rng.next() * 2.2;
        let task = r.tasks[task_idx % r.tasks.len()];
        self.agents.push(Agent {
            idx: i,
            repo,
            stack: r.stack,
            rng,
            ask: Rng::new(&self.day, &format!("ask{i}")),
            k: 0,
            task_idx,
            task,
            merged: 0,
            steps,
            done: 0,
            state: AState::Work,
            timer,
            req: None,
            stall_t: 0.0,
            last: format!("starting {task}"),
        });
        self.events.push(Ev::Joined(self.agents.len() - 1));
    }

    fn fill(&self, text: &str, a: usize) -> String {
        let ag = &self.agents[a];
        let branch = format!("{}{}", if ag.repo == "web-app" { "feat/" } else { "fix/" }, slug(ag.task));
        let msg = format!("{}{}", if ag.repo == "docs" { "docs: " } else { "feat: " }, ag.task);
        text.replacen("{branch}", &branch, 1).replacen("{msg}", &msg, 1)
    }

    #[allow(clippy::too_many_arguments)]
    fn new_req(&mut self, a: usize, tool: Tool, text: &str, detail: &'static str, danger: bool, why: &'static str, rule: Option<String>, note: &'static str) -> Req {
        self.seq += 1;
        let p = self.patience();
        Req { id: self.seq, agent: a, tool, text: self.fill(text, a), detail, danger, why, rule, note, born: self.t, patience: p, left: p, wave: 0, auto: None }
    }

    fn make_request(&mut self, a: usize) -> Req {
        let (k, idx, stack) = {
            let ag = &mut self.agents[a];
            ag.k += 1;
            (ag.k - 1, ag.idx, ag.stack)
        };
        let p_danger = (0.07 + 0.014 * f64::from(k) + 0.008 * idx as f64).min(0.3);
        let r = &mut self.agents[a].ask;
        let sneaky = r.next() < ((f64::from(k) - 2.0) * 0.09).clamp(0.0, 0.65);
        let danger = r.next() < p_danger;
        if danger {
            let fits: Vec<&content::Danger> = DANGER.iter().filter(|x| x.stack == stack || x.stack == "any").collect();
            let pool: Vec<&content::Danger> = fits.iter().copied().filter(|x| x.sneaky == sneaky).collect();
            let it = *r.pick(if pool.is_empty() { &fits } else { &pool });
            self.new_req(a, it.tool, it.text, it.detail, true, it.why, rule_for(it.tool, it.text), "")
        } else {
            let fits: Vec<&content::Safe> = SAFE.iter().filter(|x| x.stack == stack || x.stack == "any").collect();
            let pool: Vec<&content::Safe> = fits.iter().copied().filter(|x| x.sneaky == sneaky).collect();
            let it = *r.pick(if pool.is_empty() { &fits } else { &pool });
            self.new_req(a, it.tool, it.text, it.detail, false, "", content::safe_rule(it), it.note)
        }
    }

    /// The agent asks: an Always rule that covers it answers in a moment; otherwise it joins the queue.
    fn raise(&mut self, a: usize, mut req: Req) {
        self.agents[a].state = AState::Wait;
        self.agents[a].req = Some(req.id);
        if req.tool == Tool::Bash {
            if let Some(rule) = self.rules.iter().find(|r| rule_match(r, &req.text)) {
                req.auto = Some(rule.clone());
                self.autos.push(req);
                return;
            }
        }
        self.queue.push(req);
        if self.queue.len() == 1 {
            self.cur = 0;
        }
        self.events.push(Ev::Asked(a));
    }

    /// The card moves to the next (`d` 1) or previous (-1) request.
    pub fn step(&mut self, d: i32) {
        let n = self.queue.len() as i32;
        if n > 1 {
            self.cur = ((self.cur as i32 + d + n) % n) as usize;
        }
    }

    /// An answer to the request on the card.
    pub fn answer(&mut self, kind: Answer) {
        if !self.playing() || self.queue.is_empty() {
            return;
        }
        let req = self.queue[self.cur.min(self.queue.len() - 1)].clone();
        match kind {
            Answer::Later => {
                if self.queue.len() < 2 {
                    self.events.push(Ev::Nudge);
                    return;
                }
                let it = self.queue.remove(self.cur);
                self.queue.push(it);
                if self.cur >= self.queue.len() - 1 {
                    self.cur = 0;
                }
                self.events.push(Ev::Later);
            }
            Answer::Always => {
                let Some(rule) = req.rule.clone() else {
                    self.events.push(Ev::Nudge);
                    return;
                };
                if req.danger {
                    // it runs; the rule is not kept
                    self.settle(req.id, How::Always, 1);
                    return;
                }
                if !self.rules.contains(&rule) {
                    self.rules.push(rule.clone());
                }
                self.settle(req.id, How::Always, 1);
                self.events.push(Ev::Always(rule.clone()));
                let covered: Vec<u32> = self.queue.iter().filter(|q| q.tool == Tool::Bash && rule_match(&rule, &q.text)).map(|q| q.id).collect();
                for id in covered {
                    if let Some(q) = self.queue.iter_mut().find(|q| q.id == id) {
                        q.auto = Some(rule.clone());
                    }
                    self.settle(id, How::Auto, 1);
                }
            }
            Answer::All => {
                let same: Vec<u32> = self.queue.iter().filter(|q| q.text == req.text && q.tool == req.tool).map(|q| q.id).collect();
                if same.len() < 2 {
                    self.settle(req.id, How::Yes, 1);
                    return;
                }
                if req.danger {
                    let others: Vec<usize> = self.queue.iter().filter(|q| same.contains(&q.id) && q.id != req.id).map(|q| q.agent).collect();
                    let mut it = None;
                    for id in &same {
                        let q = self.dequeue(*id);
                        if *id == req.id {
                            it = q;
                        }
                    }
                    if let Some(it) = it {
                        self.settle_req(it, How::All, same.len());
                    }
                    for a in others {
                        let ag = &mut self.agents[a];
                        ag.state = AState::Work;
                        ag.req = None;
                        ag.timer = 2.0;
                    }
                    return;
                }
                for id in &same {
                    self.settle(*id, How::All, 1);
                }
                self.events.push(Ev::AllowedAll(same.len()));
            }
            Answer::Change => self.settle(req.id, How::Change, 1),
            Answer::Yes => self.settle(req.id, How::Yes, 1),
        }
    }

    /// Takes a request out of the queue, keeping the card on the same one where it can.
    fn dequeue(&mut self, id: u32) -> Option<Req> {
        let i = self.queue.iter().position(|q| q.id == id)?;
        let r = self.queue.remove(i);
        if self.cur > i || self.cur >= self.queue.len() {
            self.cur = self.cur.saturating_sub(usize::from(self.cur > i)) % self.queue.len().max(1);
        }
        Some(r)
    }

    /// Answers a request in the queue (or one an Always rule is answering).
    fn settle(&mut self, id: u32, how: How, many: usize) {
        if !self.playing() {
            return;
        }
        let req = match self.dequeue(id) {
            Some(r) => r,
            None => match self.autos.iter().position(|q| q.id == id) {
                Some(i) => self.autos.remove(i),
                None => return,
            },
        };
        self.settle_req(req, how, many);
    }

    /// Answers a request already out of the queue.
    fn settle_req(&mut self, req: Req, how: How, many: usize) {
        if !self.playing() {
            return;
        }
        let a = req.agent;
        self.agents[a].req = None;
        let w = self.t - req.born;
        self.waited += w;
        if how == How::Change {
            if req.danger {
                self.catch(&req, w);
            } else {
                self.needless(&req);
            }
            self.agents[a].state = AState::Work;
            self.agents[a].timer = self.work_time(a);
            self.agents[a].last = "redoing the step".to_string();
            return;
        }
        if req.danger {
            self.incident(&req, how, many);
            let r = 2.0 + self.agents[a].rng.next() * 2.0;
            self.agents[a].state = AState::Work;
            self.agents[a].timer = r;
            self.agents[a].last = format!("{} {}", req.tool.did(), req.text);
            return;
        }
        self.approve(&req, how, w);
    }

    fn approve(&mut self, req: &Req, how: How, w: f64) {
        let a = req.agent;
        let auto = how == How::Auto;
        let quick = !auto && w < 1.6;
        let pts = if auto { 5 } else { 10 } * self.mult() + if quick { 5 } else { 0 };
        self.score += pts;
        if !auto {
            self.combo += 1;
            self.max_combo = self.max_combo.max(self.combo);
            self.marks.push(Mark::Yes);
        }
        self.events.push(Ev::Approved { agent: a, pts, how, text: req.text.clone(), rule: req.auto.clone() });
        self.agents[a].last = format!("{} {}", req.tool.did(), req.text);
        self.agents[a].done += 1;
        if self.agents[a].done >= self.agents[a].steps {
            self.ship(a);
        } else {
            self.agents[a].state = AState::Work;
            self.agents[a].timer = self.work_time(a);
        }
    }

    fn ship(&mut self, a: usize) {
        let pts = 50 * self.mult();
        self.score += pts;
        self.shipped += 1;
        let task = self.agents[a].task;
        self.events.push(Ev::Merged { agent: a, pts, task });
        let r = content::repo(self.agents[a].repo);
        let ag = &mut self.agents[a];
        ag.task_idx += 1;
        ag.task = r.tasks[ag.task_idx % r.tasks.len()];
        ag.merged += 1;
        ag.done = 0;
        ag.steps = 3 + (ag.rng.next() * 3.0).floor() as u32;
        ag.state = AState::Work;
        ag.timer = 1.4 + ag.rng.next() * 1.5;
        ag.last = format!("starting {}", ag.task);
    }

    fn catch(&mut self, req: &Req, w: f64) {
        let pts = 40 * self.mult() + if w < 2.0 { 10 } else { 0 };
        self.score += pts;
        self.caught += 1;
        self.combo += 1;
        self.max_combo = self.max_combo.max(self.combo);
        self.marks.push(Mark::Caught);
        self.events.push(Ev::Caught { agent: req.agent, pts, text: req.text.clone(), why: req.why });
    }

    fn needless(&mut self, req: &Req) {
        self.combo = 0;
        self.needless += 1;
        self.marks.push(Mark::Slip);
        self.events.push(Ev::Needless { agent: req.agent, text: req.text.clone(), note: req.note });
    }

    fn incident(&mut self, req: &Req, how: How, many: usize) {
        self.incidents += 1;
        self.combo = 0;
        self.marks.push(Mark::Incident);
        self.shields = self.shields.saturating_sub(1);
        self.events.push(Ev::Incident { agent: req.agent, text: req.text.clone(), why: req.why, how, many, rule: req.auto.clone() });
        self.lesson = match (&req.auto, how) {
            (Some(rule), How::Auto) => format!("An Always rule as wide as {rule} also matches {}.", req.text),
            _ => format!("{}: {}", req.text, req.why),
        };
        if self.shields == 0 {
            self.finish(End::Incident);
        }
    }

    fn stall(&mut self, id: u32) {
        let Some(req) = self.dequeue(id) else { return };
        let a = req.agent;
        let ag = &mut self.agents[a];
        ag.req = None;
        ag.state = AState::Stall;
        ag.stall_t = STALL_SECS;
        self.waited += self.t - req.born;
        self.stalls += 1;
        self.combo = 0;
        self.marks.push(Mark::Slip);
        self.shields = self.shields.saturating_sub(1);
        self.events.push(Ev::Stalled { agent: a, text: req.text.clone() });
        if self.shields == 0 {
            self.finish(End::Stall);
        }
    }

    fn finish(&mut self, why: End) {
        if !self.playing() {
            return;
        }
        self.end = Some(why);
        let t = self.t;
        let still: f64 = self.agents.iter().filter_map(|a| a.req).filter_map(|id| self.queue.iter().chain(self.autos.iter()).find(|q| q.id == id)).map(|q| t - q.born).sum();
        self.waited += still;
        self.queue.clear();
        self.autos.clear();
        self.events.push(Ev::Over(why));
    }

    /// Several agents ask the very same thing at once; after 80 s some of these are traps.
    fn wave(&mut self) {
        self.wave_n += 1;
        let mut r = Rng::new(&self.day, &format!("wave{}", self.wave_n));
        let free: Vec<usize> = (0..self.agents.len()).filter(|&i| self.agents[i].state == AState::Work).collect();
        // the stack with the most free agents (the first seen wins a tie)
        let mut by: Vec<(&str, Vec<usize>)> = Vec::new();
        for &i in &free {
            let st = self.agents[i].stack;
            match by.iter_mut().find(|(s, _)| *s == st) {
                Some((_, v)) => v.push(i),
                None => by.push((st, vec![i])),
            }
        }
        by.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
        let mut group = by.first().map(|(_, v)| v.clone()).unwrap_or_default();
        let trap = self.t > 80.0 && r.next() < 0.34;
        if group.len() < 2 {
            group = free.iter().copied().take(3).collect();
        }
        if group.len() < 2 {
            return;
        }
        group.truncate(4);
        let st0 = self.agents[group[0]].stack;
        let stack = if group.iter().all(|&i| self.agents[i].stack == st0) { st0 } else { "any" };
        let text = if trap {
            let list: Vec<&str> = content::wave_traps(stack).iter().chain(content::wave_traps("any")).copied().collect();
            *r.pick(&list)
        } else {
            let w = content::waves(stack);
            *r.pick(if w.is_empty() { content::waves("any") } else { w })
        };
        let n = group.len();
        for a in group {
            let mut req = if trap {
                let d = DANGER.iter().find(|d| d.text == text).expect("a trap is in the table");
                self.new_req(a, d.tool, d.text, d.detail, true, d.why, rule_for(d.tool, d.text), "")
            } else {
                match SAFE.iter().find(|s| s.text == text) {
                    Some(s) => self.new_req(a, s.tool, s.text, s.detail, false, "", content::safe_rule(s), s.note),
                    None => self.new_req(a, Tool::Bash, text, "", false, "", Some(text.to_string()), ""),
                }
            };
            req.wave = n;
            req.patience = self.patience() + 2.0;
            req.left = req.patience;
            self.raise(a, req);
        }
        self.events.push(Ev::Wave { n, trap });
    }

    /// Moves the run on by `dt` seconds.
    pub fn update(&mut self, dt: f64) {
        if !self.playing() {
            return;
        }
        self.t += dt;
        while self.joined < ROSTER.len() && self.t >= JOIN_AT[self.joined] {
            self.add_agent(self.joined);
            self.joined += 1;
        }
        if self.t >= self.next_wave {
            self.wave();
            self.next_wave += 26.0 + Rng::new(&self.day, &format!("gap{}", self.wave_n)).next() * 14.0;
        }
        for a in 0..self.agents.len() {
            match self.agents[a].state {
                AState::Work => {
                    self.agents[a].timer -= dt;
                    if self.agents[a].timer <= 0.0 {
                        let req = self.make_request(a);
                        self.raise(a, req);
                    }
                }
                AState::Stall => {
                    self.agents[a].stall_t -= dt;
                    if self.agents[a].stall_t <= 0.0 {
                        self.agents[a].state = AState::Work;
                        self.agents[a].timer = self.work_time(a);
                        self.agents[a].last = "back to work".to_string();
                    }
                }
                AState::Wait => {}
            }
        }
        let due: Vec<u32> = self.autos.iter().filter(|q| self.t - q.born >= AUTO_SECS).map(|q| q.id).collect();
        for id in due {
            self.settle(id, How::Auto, 1);
        }
        let mut late = Vec::new();
        for q in &mut self.queue {
            q.left -= dt;
            if q.left <= 0.0 {
                late.push(q.id);
            }
        }
        for id in late {
            if self.playing() {
                self.stall(id);
            }
        }
    }

    /// The result to paste anywhere: the run, the score, the last 24 answers as squares, the wait.
    pub fn share_text(&self, day: &Day, url: &str) -> String {
        let n = self.marks.len();
        let marks: String = self.marks[n.saturating_sub(24)..].iter().map(|m| m.emoji()).collect();
        let mut lines = vec![format!("Hold the Swarm · {}", day.run_label()), format!("{} pts · {} shipped · {} caught", thousands(self.score), self.shipped, self.caught)];
        if !marks.is_empty() {
            lines.push(marks);
        }
        lines.push(format!("Agents waited {:.1} s on me", self.waited));
        if !url.is_empty() {
            lines.push(url.to_string());
        }
        lines.join("\n")
    }
}

/// `12,340`
pub fn thousands(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------- a player that isn't you

/// How the bot plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// blocks what's dangerous, says yes to the rest, saves only narrow rules, answers waves at once
    Expert,
    /// the same, slower, and wrong now and then
    Casual,
    /// Always on everything that offers it
    #[cfg_attr(not(test), allow(dead_code))]
    Greedy,
    /// Enter on everything
    #[cfg_attr(not(test), allow(dead_code))]
    Reckless,
}

/// A player for the tests, the look tests and the README clip: it reads the card, waits its reaction time, answers.
#[derive(Clone, Debug)]
pub struct Bot {
    pub style: Style,
    react: f64,
    errors: f64,
    rng: Rng,
    seen: Option<u32>,
    wait: f64,
}

impl Bot {
    pub fn new(style: Style, day: &str) -> Bot {
        let (react, errors) = match style {
            Style::Expert => (0.65, 0.0),
            Style::Casual => (1.25, 0.08),
            Style::Greedy => (0.8, 0.0),
            Style::Reckless => (0.5, 0.0),
        };
        Bot { style, react, errors, rng: Rng::new(day, "bot"), seen: None, wait: 0.0 }
    }

    /// The answer it gives this tick, if any.
    pub fn act(&mut self, run: &Run, dt: f64) -> Option<Answer> {
        let req = run.current()?;
        if self.seen != Some(req.id) {
            self.seen = Some(req.id);
            self.wait = self.react * (0.7 + 0.6 * self.rng.next());
        }
        self.wait -= dt;
        if self.wait > 0.0 {
            return None;
        }
        self.seen = None;
        let wrong = self.rng.next() < self.errors;
        let danger = req.danger != wrong;
        Some(match self.style {
            Style::Reckless => Answer::Yes,
            Style::Greedy if req.rule.is_some() => Answer::Always,
            Style::Greedy => Answer::Yes,
            _ if danger => Answer::Change,
            _ if run.same_count(req) > 1 => Answer::All,
            Style::Expert if req.rule.as_deref().is_some_and(content::rule_is_safe) => Answer::Always,
            _ => Answer::Yes,
        })
    }
}

/// Plays a whole run with a bot at `dt` a tick (up to `max` seconds); the run as it ended.
#[cfg(test)]
pub fn play_out(day: &str, style: Style, dt: f64, max: f64) -> Run {
    let mut run = Run::new(day);
    let mut bot = Bot::new(style, day);
    while run.playing() && run.t < max {
        run.update(dt);
        if let Some(a) = bot.act(&run, dt) {
            run.answer(a);
        }
        run.events.clear();
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_day_names_the_run() {
        let d = Day::at(1_792_454_400); // 2026-10-20 00:00 UTC
        assert_eq!((d.key.as_str(), d.run, d.label.as_str()), ("2026-10-20", 1, "20 OCT"));
        let d = Day::at(1_792_454_400 + 86_400 * 12 + 3_600);
        assert_eq!((d.key.as_str(), d.run, d.run_label().as_str()), ("2026-11-01", 13, "Run 013"));
        assert_eq!(civil_from_days(days_from_civil(2028, 2, 29)), (2028, 2, 29));
        assert_eq!(Day::default(), Day::first());
    }

    #[test]
    fn the_generator_matches_the_prototype() {
        // the prototype's `seedOf` and `rng` (mulberry32), run in Node
        assert_eq!(seed_of("2026-10-20:agent0"), 2_987_459_801);
        assert_eq!(seed_of("2026-10-20:ask3"), 267_355_528);
        let near = |r: &mut Rng, want: &[f64]| {
            for w in want {
                let x = r.next();
                assert!((x - w).abs() < 1e-11, "{x} vs {w}");
            }
        };
        near(&mut Rng::new("2026-10-20", "agent0"), &[0.467033287045, 0.775243249489, 0.109931688756, 0.102448798250]);
        near(&mut Rng::new("2026-10-21", "wave1"), &[0.449199269060, 0.418536749668, 0.842349215876]);
    }

    #[test]
    fn each_agent_asks_what_it_asks_in_the_browser() {
        // the prototype's first eight asks for agents 0, 3 and 5 on the first day, run in Node ("!" is dangerous)
        let want: [(usize, [&str; 8]); 3] = [
            (0, ["rg \"TODO\" src", "!crontab -r", "!crontab -r", "git clean -n", "git clean -n", "rm -rf node_modules && npm ci", "npm install", "!~/.zshrc"]),
            (3, ["git checkout -b fix/staging-database", "git log --oneline -5", "git commit -m \"feat: staging database\"", "!~/.ssh/authorized_keys", "!chmod -R 777 ~", "git push --force-with-lease origin fix/staging-database", "git log --oneline -5", "kubectl get pods -n staging"]),
            (5, ["git log --oneline -5", "git push origin fix/retry-on-503", "go vet ./...", "ls -la src", "rg \"TODO\" src", "git clean -n", "!chmod -R 777 ~", "git clean -n"]),
        ];
        let mut r = Run::new("2026-10-20");
        for i in 3..ROSTER.len() {
            r.add_agent(i);
        }
        for (idx, asks) in want {
            let a = r.agents.iter().position(|g| g.idx == idx).unwrap();
            let got: Vec<String> = (0..8).map(|_| r.make_request(a)).map(|q| format!("{}{}", if q.danger { "!" } else { "" }, q.text)).collect();
            assert_eq!(got, asks, "agent {idx}");
        }
    }

    #[test]
    fn everyone_gets_the_same_asks_that_day() {
        let a = play_out("2026-10-20", Style::Expert, 1.0 / 60.0, 60.0);
        let b = play_out("2026-10-20", Style::Expert, 1.0 / 60.0, 60.0);
        assert_eq!(a.score, b.score);
        assert_eq!(a.marks, b.marks);
        let c = play_out("2026-10-21", Style::Expert, 1.0 / 60.0, 60.0);
        assert_ne!((a.score, a.marks.len()), (c.score, c.marks.len()));
    }

    #[test]
    fn three_agents_start_and_the_rest_join_on_time() {
        let mut r = Run::new("2026-10-20");
        assert_eq!(r.agents.len(), 3);
        while r.t < 16.0 {
            r.update(0.05);
            // answer everything so nobody stalls
            while r.current().is_some() {
                let danger = r.current().unwrap().danger;
                r.answer(if danger { Answer::Change } else { Answer::Yes });
            }
        }
        assert_eq!(r.agents.len(), 4);
        assert_eq!(r.agents[3].repo, "infra");
    }

    #[test]
    fn a_yes_scores_and_the_last_step_merges() {
        let mut r = Run::new("2026-10-20");
        while r.queue.is_empty() {
            r.update(0.05);
        }
        let a = r.current().unwrap().agent;
        let steps = r.agents[a].steps;
        let mut merged = false;
        for _ in 0..steps {
            while !r.queue.iter().any(|q| q.agent == a) {
                r.update(0.05);
                r.queue.retain(|q| q.agent == a);
            }
            let i = r.queue.iter().position(|q| q.agent == a).unwrap();
            r.cur = i;
            r.queue[i].danger = false;
            r.answer(Answer::Yes);
            merged |= r.events.iter().any(|e| matches!(e, Ev::Merged { agent, .. } if *agent == a));
        }
        assert!(merged);
        assert_eq!(r.shipped, 1);
        assert_eq!(r.agents[a].merged, 1);
        assert!(r.score >= 50 + steps * 10);
    }

    fn with_one(text: &str, danger: bool, rule: Option<&str>) -> Run {
        let mut r = Run::new("2026-10-20");
        let req = Req { id: 99, agent: 0, tool: Tool::Bash, text: text.into(), detail: "", danger, why: "bad", rule: rule.map(str::to_string), note: "", born: 0.0, patience: 9.0, left: 9.0, wave: 0, auto: None };
        r.raise(0, req);
        r
    }

    #[test]
    fn blocking_a_dangerous_ask_catches_it_and_a_safe_one_breaks_the_combo() {
        let mut r = with_one("rm -rf ~", true, Some("rm -rf *"));
        r.combo = 7;
        r.answer(Answer::Change);
        assert_eq!((r.caught, r.combo, r.shields), (1, 8, 3));
        assert_eq!(r.marks, vec![Mark::Caught]);
        let mut r = with_one("npm test", false, Some("npm test"));
        r.combo = 7;
        r.answer(Answer::Change);
        assert_eq!((r.needless, r.combo), (1, 0));
    }

    #[test]
    fn letting_a_dangerous_ask_through_costs_a_shield_and_the_third_ends_the_run() {
        let mut r = with_one("rm -rf ~", true, None);
        r.answer(Answer::Yes);
        assert_eq!((r.incidents, r.shields), (1, 2));
        assert!(r.lesson.contains("rm -rf ~"));
        r.shields = 1;
        r.raise(0, Req { id: 100, agent: 0, tool: Tool::Bash, text: "crontab -r".into(), detail: "", danger: true, why: "x", rule: None, note: "", born: 0.0, patience: 9.0, left: 9.0, wave: 0, auto: None });
        r.answer(Answer::Yes);
        assert_eq!(r.end, Some(End::Incident));
        assert!(r.events.contains(&Ev::Over(End::Incident)));
    }

    #[test]
    fn an_always_rule_answers_what_it_covers_even_the_bad_ones() {
        let mut r = with_one("git push origin fix/x", false, Some("git push *"));
        r.answer(Answer::Always);
        assert_eq!(r.rules, vec!["git push *".to_string()]);
        // the next push, a force push to main, is answered by the rule before it reaches the card
        r.raise(1, Req { id: 101, agent: 1, tool: Tool::Bash, text: "git push --force origin main".into(), detail: "", danger: true, why: "It rewrites main for everyone.", rule: Some("git push *".into()), note: "", born: r.t, patience: 9.0, left: 9.0, wave: 0, auto: None });
        assert!(r.queue.is_empty());
        r.update(0.3);
        assert_eq!(r.incidents, 1);
        assert!(r.lesson.contains("git push *"), "{}", r.lesson);
    }

    #[test]
    fn allow_all_answers_every_agent_asking_the_same() {
        let mut r = with_one("npm test", false, Some("npm test"));
        r.raise(1, Req { id: 102, agent: 1, tool: Tool::Bash, text: "npm test".into(), detail: "", danger: false, why: "", rule: Some("npm test".into()), note: "", born: 0.0, patience: 9.0, left: 9.0, wave: 2, auto: None });
        assert_eq!(r.same_count(r.current().unwrap()), 2);
        r.answer(Answer::All);
        assert!(r.queue.is_empty());
        assert!(r.events.contains(&Ev::AllowedAll(2)));
        assert_eq!(r.combo, 2);
    }

    #[test]
    fn a_request_left_too_long_stalls_its_agent() {
        let mut r = with_one("npm test", false, None);
        r.update(9.5);
        assert_eq!((r.stalls, r.shields), (1, 2));
        assert_eq!(r.agents[0].state, AState::Stall);
        r.update(STALL_SECS + 0.1);
        assert_eq!(r.agents[0].state, AState::Work);
    }

    #[test]
    fn later_moves_the_ask_to_the_back_and_the_arrows_step() {
        let mut r = with_one("npm test", false, None);
        r.answer(Answer::Later);
        assert_eq!(r.events.last(), Some(&Ev::Nudge));
        r.raise(1, Req { id: 103, agent: 1, tool: Tool::Bash, text: "cargo test".into(), detail: "", danger: false, why: "", rule: None, note: "", born: 0.0, patience: 9.0, left: 9.0, wave: 0, auto: None });
        r.answer(Answer::Later);
        assert_eq!(r.queue.iter().map(|q| q.id).collect::<Vec<_>>(), vec![103, 99]);
        r.step(1);
        assert_eq!(r.current().unwrap().id, 99);
        r.step(1);
        assert_eq!(r.current().unwrap().id, 103);
    }

    #[test]
    fn the_bots_play_like_the_tuned_prototype() {
        // an expert holds the swarm for minutes; Always on everything lets a bad one through; Enter on everything
        // goes down fastest
        let dt = 1.0 / 60.0;
        let mut expert = 0.0;
        let mut greedy = 0.0;
        let mut reckless = 0.0;
        let days = ["2026-10-20", "2026-10-21", "2026-10-22", "2026-10-23", "2026-10-24"];
        for d in days {
            let e = play_out(d, Style::Expert, dt, 900.0);
            let g = play_out(d, Style::Greedy, dt, 900.0);
            let r = play_out(d, Style::Reckless, dt, 900.0);
            assert!(g.incidents > 0, "{d}: greedy never slipped");
            expert += e.t;
            greedy += g.t;
            reckless += r.t;
        }
        let n = days.len() as f64;
        let (expert, greedy, reckless) = (expert / n, greedy / n, reckless / n);
        assert!(expert > 120.0, "expert {expert}");
        assert!(expert > greedy && greedy > reckless * 0.8, "expert {expert} greedy {greedy} reckless {reckless}");
        assert!(reckless < 120.0, "reckless {reckless}");
    }

    #[test]
    fn the_result_reads_as_a_post() {
        let mut r = Run::new("2026-10-20");
        r.score = 12_340;
        r.shipped = 4;
        r.caught = 2;
        r.waited = 12.34;
        r.marks = vec![Mark::Yes, Mark::Caught, Mark::Incident, Mark::Slip];
        let t = r.share_text(&Day::first(), "https://example.com");
        assert_eq!(t, "Hold the Swarm · Run 001\n12,340 pts · 4 shipped · 2 caught\n🟩🟦🟥🟨\nAgents waited 12.3 s on me\nhttps://example.com");
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(1_000), "1,000");
    }

    #[test]
    fn fill_names_the_branch_and_the_commit() {
        let r = Run::new("2026-10-20");
        assert_eq!(r.fill("git push origin {branch}", 0), "git push origin feat/add-dark-mode");
        assert_eq!(r.fill("git commit -m \"{msg}\"", 1), "git commit -m \"feat: rate limiter\"");
    }
}
