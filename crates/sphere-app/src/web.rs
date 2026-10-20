//! The web playground's stand-ins for what a browser doesn't have (`scripts/build-web.sh`).

pub mod sim;

/// The page's query as the app's flags: `?swarm&theme=red` reads as `--swarm --theme red`.
pub fn args() -> Vec<String> {
    let q = web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default();
    let mut out = Vec::new();
    for part in q.trim_start_matches('?').split('&').filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        out.push(format!("--{}", k.trim_start_matches('-')));
        if !v.is_empty() {
            out.push(v.to_string());
        }
    }
    out
}

/// tokio's unbounded channel, for the link that has no tokio here (nothing reads it: there's no sphered).
pub mod mpsc {
    pub use std::sync::mpsc::{Receiver as UnboundedReceiver, Sender as UnboundedSender};
    pub fn unbounded_channel<T>() -> (UnboundedSender<T>, UnboundedReceiver<T>) {
        std::sync::mpsc::channel()
    }
}

/// Local time as "13:34" and "FRIDAY, OCTOBER 16, 2026", in the browser's zone (`hud::clock_strings`).
pub fn clock_strings() -> (String, String) {
    const DAYS: [&str; 7] = ["SUNDAY", "MONDAY", "TUESDAY", "WEDNESDAY", "THURSDAY", "FRIDAY", "SATURDAY"];
    const MONTHS: [&str; 12] = ["JANUARY", "FEBRUARY", "MARCH", "APRIL", "MAY", "JUNE", "JULY", "AUGUST", "SEPTEMBER", "OCTOBER", "NOVEMBER", "DECEMBER"];
    let d = js_sys::Date::new_0();
    (
        format!("{:02}:{:02}", d.get_hours(), d.get_minutes()),
        format!("{}, {} {}, {}", DAYS[d.get_day() as usize % 7], MONTHS[d.get_month() as usize % 12], d.get_date(), d.get_full_year()),
    )
}

/// The playground's terminal tab: a pretend zsh in the sphere repo. It echoes what you type and answers a few
/// commands with what they print on a Mac, wrapped in the same marks as Sphere's shell integration, so a failed
/// `cargo test` brings up Fix it like a real tab does.
pub struct Shell {
    tx: std::sync::mpsc::Sender<Vec<u8>>,
    line: String,
    /// skipping an escape sequence (arrow keys and the like): 1 after ESC, 2 inside CSI
    esc: u8,
}

const CYAN: &str = "\x1b[38;2;111;216;240m";
const LINE: &str = "\x1b[38;2;166;211;230m";
const AMBER: &str = "\x1b[38;2;255;179;71m";
const OFF: &str = "\x1b[0m";
const DIR: &str = "file://mac.local/Users/you/Projects/sphere";

fn osc(body: &str) -> String {
    format!("\x1b]{body}\x07")
}

impl Shell {
    pub fn new(tx: std::sync::mpsc::Sender<Vec<u8>>, first: Option<&str>) -> Self {
        let mut sh = Shell { tx, line: String::new(), esc: 0 };
        sh.out(&format!("{}Sphere playground · a pretend shell. Try {AMBER}help{OFF}, {AMBER}git status{OFF} or {AMBER}cargo test{OFF}.\r\n\r\n", LINE));
        sh.prompt();
        if let Some(cmd) = first {
            sh.out(cmd);
            sh.run(cmd);
        }
        sh
    }

    fn out(&self, s: &str) {
        let _ = self.tx.send(s.as_bytes().to_vec());
    }

    fn prompt(&self) {
        let nonce = crate::term::SCRIPT_NONCE;
        self.out(&format!("{}{}{CYAN}you{OFF} {LINE}sphere{OFF} {AMBER}›{OFF} ", osc(&format!("7;{DIR}")), osc(&format!("133;A;sphere={nonce}"))));
    }

    fn run(&mut self, cmd: &str) {
        let nonce = crate::term::SCRIPT_NONCE;
        let cmd = cmd.trim();
        self.out("\r\n");
        if cmd.is_empty() {
            self.prompt();
            return;
        }
        if cmd == "clear" {
            self.out("\x1b[2J\x1b[H");
            self.prompt();
            return;
        }
        self.out(&format!("{}{}", osc(&format!("7700;cmd;{};{nonce}", crate::fixit::base64(cmd))), osc(&format!("133;C;sphere={nonce}"))));
        let (text, status) = answer(cmd);
        if status == 101 {
            // cargo test: the same run as the look test's, failing test and all
            let (start, _) = crate::fixit::demo_parts();
            let s = String::from_utf8_lossy(&start).into_owned();
            let body = s.split_once("133;C;sphere=").and_then(|(_, r)| r.split_once('\x07')).map(|(_, r)| r.to_string()).unwrap_or_default();
            self.out(&body);
        } else {
            self.out(&text.replace('\n', "\r\n"));
        }
        self.out(&format!("{}{}", osc(&format!("7;{DIR}")), osc(&format!("133;D;{status};sphere={nonce}"))));
        self.prompt();
    }
}

impl std::io::Write for Shell {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(b).into_owned();
        for c in text.chars() {
            match (self.esc, c) {
                (1, '[') | (1, 'O') => self.esc = 2,
                (1, _) => self.esc = 0,
                (2, c) if c.is_ascii_alphabetic() || c == '~' => self.esc = 0,
                (2, _) => {}
                (_, '\x1b') => self.esc = 1,
                (_, '\r') | (_, '\n') => {
                    let line = std::mem::take(&mut self.line);
                    self.run(&line);
                }
                (_, '\x7f') | (_, '\x08') => {
                    if self.line.pop().is_some() {
                        self.out("\x08 \x08");
                    }
                }
                (_, '\x03') => {
                    self.line.clear();
                    self.out("^C\r\n");
                    self.prompt();
                }
                (_, '\x0c') => {
                    self.out("\x1b[2J\x1b[H");
                    self.prompt();
                    self.out(&self.line.clone());
                }
                (_, c) if !c.is_control() => {
                    self.line.push(c);
                    self.out(&c.to_string());
                }
                _ => {}
            }
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// What the pretend shell prints for `cmd`, and its exit status (101: the failing `cargo test`).
fn answer(cmd: &str) -> (String, i32) {
    let (y, g, r, c, d, b) = ("\x1b[33m", "\x1b[32m", "\x1b[31m", "\x1b[36m", "\x1b[2m", "\x1b[1m");
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let s = match words.as_slice() {
        ["help"] => format!(
            "{b}This is a pretend shell{OFF} inside the Sphere playground. It knows:\n  {AMBER}ls{OFF}  {AMBER}pwd{OFF}  {AMBER}git status{OFF}  {AMBER}git log{OFF}  {AMBER}cargo test{OFF}  {AMBER}docker ps{OFF}  {AMBER}clear{OFF}\n\n\
             {b}cargo test{OFF} fails one test: press {AMBER}Cmd/Ctrl+Enter{OFF} on the strip that comes up, and an agent fixes it.\n\
             In the app this is your real shell (zsh, bash), and its tabs outlive Sphere.\n"
        ),
        ["ls", ..] => format!("CODE_OF_CONDUCT.md  Cargo.lock  LICENSE  README.md  {c}assets{OFF}  {c}crates{OFF}  {c}scripts{OFF}\nCONTRIBUTING.md     Cargo.toml  NOTICE   SECURITY.md  {c}docs{OFF}    {c}target{OFF}\n"),
        ["pwd"] => "/Users/you/Projects/sphere\n".to_string(),
        ["whoami"] => "you\n".to_string(),
        ["echo", rest @ ..] => format!("{}\n", rest.join(" ")),
        ["git", "status", ..] => format!("On branch {g}main{OFF}\nYour branch is up to date with 'origin/main'.\n\nChanges not staged for commit:\n  (use \"git add <file>...\" to update what will be committed)\n\t{r}modified:   crates/sphered/src/workspace.rs{OFF}\n\nno changes added to commit (use \"git add\" and/or \"git commit -a\")\n"),
        ["git", "log", ..] => {
            let log = [
                ("f99e142", "(HEAD -> main, tag: v0.1.4, origin/main)", "Sphere 0.1.4 (#28)"),
                ("3309437", "", "Memory, Protocols and Fix it: reasons to come back to Sphere every day (#27)"),
                ("8ec3e2a", "", "README: every clip at Fast speed, the swarm, and what landed since 0.1.3 (#26)"),
                ("d4ccb6f", "", "Look for outside sessions with ps alone while nothing changes (#25)"),
                ("edaa9b8", "", "Speed-up: far less CPU, GPU and memory for the same Sphere (#16)"),
                ("19e70b4", "", "Show Calendar events in the home's Next 24h rail (#22)"),
                ("6215be1", "", "Agents wait on you less: Always allow, Allow all, notification buttons (#23)"),
                ("2b18150", "", "Hide Music and the command bar's mic button (#24)"),
                ("5b0873f", "", "Restart an out-of-date sphered after an update (#21)"),
                ("f74cdf3", "", "Take over a Claude session running in another terminal (#20)"),
                ("d1a7a5c", "", "Git PRs: action buttons get their own row (#19)"),
                ("1cf3723", "", "Code opens folders, edits in Helix or Zed (#14)"),
                ("d1a2e05", "", "Make the home's placeholders real (#17)"),
                ("8546bf3", "", "Questions end as Answered with no branch; F follows up an agent (#18)"),
                ("f766a17", "", "Git shows your pull requests (#15)"),
            ];
            log.iter().map(|(h, refs, m)| if refs.is_empty() { format!("{r}*{OFF} {y}{h}{OFF} {m}\n") } else { format!("{r}*{OFF} {y}{h} {c}{refs}{OFF} {m}\n") }).collect()
        }
        ["git", ..] => format!("{d}(the playground's git knows status and log; the Git app has the rest){OFF}\n"),
        ["cargo", "test", ..] => return (String::new(), 101),
        ["cargo", ..] => format!("{g}{b}    Finished{OFF} `dev` profile [optimized + debuginfo] target(s) in 0.41s\n"),
        ["docker", "ps", ..] => format!(
            "CONTAINER ID   IMAGE                COMMAND                  STATUS          PORTS                    NAMES\n\
             7a1c9e02f3b4   postgres:16          \"docker-entrypoint.s…\"   Up 2 hours      0.0.0.0:5432->5432/tcp   shop-db\n\
             c4d8e1a90b27   redis:7-alpine       \"docker-entrypoint.s…\"   Up 2 hours      0.0.0.0:6379->6379/tcp   shop-cache\n\
             e93f0b6d2a15   shop-api:dev         \"cargo run --release\"    Up 14 minutes   0.0.0.0:8080->8080/tcp   shop-api\n"
        ),
        ["claude", ..] | ["sphere", ..] => format!("Start an agent from the command bar: press {AMBER}/{OFF}, type a task, Enter.\n"),
        [w, ..] => return (format!("zsh: command not found: {w}\n"), 127),
        [] => String::new(),
    };
    (s, 0)
}
