//! Shell integration for Sphere's terminal tabs: what makes a shell tell the terminal where each command starts
//! and ends (scan.rs reads it), so a failed command can offer "Fix it". zsh and bash get it; any other shell runs
//! as before. The scripts are in assets/shell, embedded here and written to ~/.sphere/shell when the first tab
//! starts. A script whose content changed is rewritten; one that didn't is left alone.
//!
//! zsh reads its startup files from ZDOTDIR, so ZDOTDIR points at Sphere's copies, which read your own files and
//! hand ZDOTDIR back. A login bash ignores --rcfile, so bash starts as `bash --rcfile <sphere's> -i` and that
//! file does what a login shell would before it adds the hooks. Both adapt VS Code's shell integration (MIT),
//! credited in the scripts. SPHERE_SHELL_INTEGRATION=0 switches it all off.
use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::OnceLock,
};

const BASH_RC: &str = include_str!("../assets/shell/bash.sh");
/// The files of ZDOTDIR, by the name zsh looks for.
const ZSH_FILES: [(&str, &str); 4] = [
    (".zshenv", include_str!("../assets/shell/zshenv")),
    (".zprofile", include_str!("../assets/shell/zprofile")),
    (".zshrc", include_str!("../assets/shell/zshrc")),
    (".zlogin", include_str!("../assets/shell/zlogin")),
];

/// How to start a shell: its arguments, and the variables to add to its environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Launch {
    /// A login shell as before, with no integration.
    fn plain() -> Self {
        Self { args: vec!["-l".into()], env: Vec::new() }
    }
}

/// How to start `shell` (a path, as in $SHELL) for a terminal tab: with the integration for zsh and bash, as
/// a plain login shell for any other, or when it is off or the scripts can't be written.
pub fn launch(shell: &str) -> Launch {
    let setting = std::env::var("SPHERE_SHELL_INTEGRATION").ok();
    if !enabled(setting.as_deref()) || !matches!(shell_name(shell), "zsh" | "bash") {
        return Launch::plain();
    }
    match scripts_dir() {
        Some(dir) => launch_in(dir, shell, std::env::var("ZDOTDIR").ok().as_deref(), setting.as_deref()),
        None => Launch::plain(),
    }
}

/// `launch` with the scripts in `dir`, the user's own `ZDOTDIR` and the SPHERE_SHELL_INTEGRATION setting given.
fn launch_in(dir: &Path, shell: &str, user_zdotdir: Option<&str>, setting: Option<&str>) -> Launch {
    if !enabled(setting) {
        return Launch::plain();
    }
    let on = ("SPHERE_SHELL_INTEGRATION".to_string(), "1".to_string());
    match shell_name(shell) {
        "zsh" => Launch {
            args: vec!["-l".into()],
            env: vec![
                ("ZDOTDIR".into(), dir.join("zsh").to_string_lossy().into_owned()),
                // the scripts hand it back once zsh has read your files
                ("SPHERE_USER_ZDOTDIR".into(), user_zdotdir.unwrap_or("").to_string()),
                on,
            ],
        },
        "bash" => Launch { args: vec!["--rcfile".into(), dir.join("bash.sh").to_string_lossy().into_owned(), "-i".into()], env: vec![on] },
        _ => Launch::plain(),
    }
}

/// `/bin/zsh` is `zsh`.
fn shell_name(shell: &str) -> &str {
    Path::new(shell).file_name().and_then(|n| n.to_str()).unwrap_or(shell)
}

/// SPHERE_SHELL_INTEGRATION is on unless it says 0, false, off or no.
fn enabled(setting: Option<&str>) -> bool {
    !matches!(setting.map(|s| s.trim().to_ascii_lowercase()).as_deref(), Some("0" | "false" | "off" | "no"))
}

/// ~/.sphere/shell with the scripts in it, written once per run; `None` when that fails (no home, no space...).
fn scripts_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        if std::env::var_os("HOME").is_none_or(|h| h.is_empty()) {
            return None;
        }
        let dir = sphere_proto::paths::default_state_dir().join("shell");
        match install(&dir) {
            Ok(()) => Some(dir),
            Err(e) => {
                eprintln!("shell integration is off, could not write {}: {e}", dir.display());
                None
            }
        }
    })
    .as_deref()
}

/// Writes the scripts into `dir` (private to you), replacing the ones whose content changed.
fn install(dir: &Path) -> io::Result<()> {
    let zsh = dir.join("zsh");
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&zsh)?;
    write_if_changed(&dir.join("bash.sh"), BASH_RC)?;
    for (name, content) in ZSH_FILES {
        write_if_changed(&zsh.join(name), content)?;
    }
    Ok(())
}

/// Writes `content` unless the file has it already. A new file replaces the old in one step, so a shell that
/// starts meanwhile reads one whole script or the other.
fn write_if_changed(path: &Path, content: &str) -> io::Result<()> {
    if fs::read_to_string(path).is_ok_and(|old| old == content) {
        return Ok(());
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let written = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| io::Write::write_all(&mut f, content.as_bytes()));
    if let Err(e) = written.and_then(|_| fs::rename(&tmp, path)) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Event, Finished, Scanner};
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::{
        io::{Read, Write},
        os::unix::fs::{MetadataExt, PermissionsExt},
        sync::mpsc,
        time::{Duration, Instant},
    };

    /// A scratch directory that goes away again.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("sphere-shellint-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(fs::canonicalize(dir).unwrap())
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn on_path(prog: &str) -> Option<PathBuf> {
        std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(prog)).find(|f| f.is_file())
    }

    #[test]
    fn zsh_gets_zdotdir_and_bash_gets_a_rcfile() {
        let dir = Path::new("/home/me/.sphere/shell");
        let z = launch_in(dir, "/bin/zsh", None, None);
        assert_eq!(z.args, ["-l"]);
        assert!(z.env.contains(&("ZDOTDIR".into(), "/home/me/.sphere/shell/zsh".into())));
        assert!(z.env.contains(&("SPHERE_USER_ZDOTDIR".into(), "".into())));
        assert!(z.env.contains(&("SPHERE_SHELL_INTEGRATION".into(), "1".into())));
        // the user's own ZDOTDIR is kept for the scripts
        let z = launch_in(dir, "/usr/bin/zsh", Some("/home/me/.config/zsh"), Some("1"));
        assert!(z.env.contains(&("SPHERE_USER_ZDOTDIR".into(), "/home/me/.config/zsh".into())));
        let b = launch_in(dir, "/opt/homebrew/bin/bash", None, None);
        assert_eq!(b.args, ["--rcfile", "/home/me/.sphere/shell/bash.sh", "-i"]);
        assert_eq!(b.env, [("SPHERE_SHELL_INTEGRATION".to_string(), "1".to_string())]);
    }

    #[test]
    fn other_shells_and_a_switched_off_setting_start_as_before() {
        let dir = Path::new("/x");
        for shell in ["/usr/local/bin/fish", "/bin/sh", "/bin/dash", "/bin/tcsh", "", "zsh-like"] {
            assert_eq!(launch_in(dir, shell, None, None), Launch::plain(), "{shell}");
        }
        for off in ["0", "false", "FALSE", "off", "no", " 0 "] {
            assert_eq!(launch_in(dir, "/bin/zsh", None, Some(off)), Launch::plain(), "{off}");
            assert_eq!(launch_in(dir, "/bin/bash", None, Some(off)), Launch::plain(), "{off}");
        }
        for on in ["1", "true", "", "yes"] {
            assert_ne!(launch_in(dir, "/bin/bash", None, Some(on)), Launch::plain(), "{on}");
        }
        assert_eq!(Launch::plain().args, ["-l"]);
    }

    #[test]
    fn install_writes_the_scripts_privately() {
        let s = Scratch::new("install");
        let dir = s.0.join("shell");
        install(&dir).unwrap();
        assert_eq!(fs::read_to_string(dir.join("bash.sh")).unwrap(), BASH_RC);
        for (name, content) in ZSH_FILES {
            assert_eq!(fs::read_to_string(dir.join("zsh").join(name)).unwrap(), content, "{name}");
            assert_eq!(fs::metadata(dir.join("zsh").join(name)).unwrap().permissions().mode() & 0o777, 0o600, "{name}");
        }
        assert_eq!(fs::metadata(dir.join("zsh")).unwrap().permissions().mode() & 0o777, 0o700);
        // nothing else is left behind
        let mut names: Vec<String> = fs::read_dir(dir.join("zsh")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, [".zlogin", ".zprofile", ".zshenv", ".zshrc"]);
    }

    #[test]
    fn install_rewrites_a_changed_script_and_leaves_the_others_alone() {
        let s = Scratch::new("rewrite");
        let dir = s.0.join("shell");
        install(&dir).unwrap();
        let ino = |p: &Path| fs::metadata(p).unwrap().ino();
        let (bash, zshrc) = (dir.join("bash.sh"), dir.join("zsh/.zshrc"));
        let bash_ino = ino(&bash);
        fs::write(&zshrc, "# an older version\n").unwrap();
        install(&dir).unwrap();
        assert_eq!(fs::read_to_string(&zshrc).unwrap(), ZSH_FILES[2].1);
        assert_eq!(fs::metadata(&zshrc).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(ino(&bash), bash_ino, "an unchanged script is not rewritten");
        let zshrc_ino = ino(&zshrc);
        install(&dir).unwrap();
        assert_eq!(ino(&zshrc), zshrc_ino);
    }

    #[test]
    fn install_fails_cleanly_where_it_cannot_write() {
        let s = Scratch::new("nowrite");
        fs::write(s.0.join("file"), "x").unwrap();
        assert!(install(&s.0.join("file").join("shell")).is_err());
    }

    #[test]
    fn the_scripts_parse() {
        let s = Scratch::new("parse");
        let dir = s.0.join("shell");
        install(&dir).unwrap();
        if let Some(bash) = on_path("bash") {
            let out = std::process::Command::new(bash).arg("-n").arg(dir.join("bash.sh")).output().unwrap();
            assert!(out.status.success(), "bash -n: {}", String::from_utf8_lossy(&out.stderr));
        }
        if let Some(zsh) = on_path("zsh") {
            for (name, _) in ZSH_FILES {
                let out = std::process::Command::new(&zsh).arg("-n").arg(dir.join("zsh").join(name)).output().unwrap();
                assert!(out.status.success(), "zsh -n {name}: {}", String::from_utf8_lossy(&out.stderr));
            }
        }
    }

    /// A real shell on a PTY: types each line after a prompt and lets the scanner read what comes back.
    struct Session {
        rx: mpsc::Receiver<Vec<u8>>,
        raw: Vec<u8>,
        scanner: Scanner,
        done: Vec<Finished>,
        writer: Box<dyn Write + Send>,
        child: Box<dyn portable_pty::Child + Send + Sync>,
    }

    fn prompts(raw: &[u8]) -> usize {
        raw.windows(8).filter(|w| *w == b"\x1b]133;A\x07").count()
    }

    impl Session {
        /// Starts `shell` the way a terminal tab would, with `home` as its home and the scripts in `dir`.
        fn start(shell: &Path, dir: &Path, home: &Path, user_zdotdir: Option<&Path>) -> Self {
            install(dir).unwrap();
            let launch = launch_in(dir, shell.to_str().unwrap(), user_zdotdir.and_then(|p| p.to_str()), None);
            let pair = native_pty_system().openpty(PtySize { rows: 24, cols: 120, pixel_width: 0, pixel_height: 0 }).unwrap();
            let mut cmd = CommandBuilder::new(shell);
            cmd.args(&launch.args);
            cmd.env_clear();
            cmd.env("PATH", std::env::var("PATH").unwrap_or_default());
            cmd.env("HOME", home);
            cmd.env("TERM", "xterm-256color");
            for (k, v) in &launch.env {
                cmd.env(k, v);
            }
            cmd.cwd(home);
            let child = pair.slave.spawn_command(cmd).unwrap();
            drop(pair.slave);
            let mut reader = pair.master.try_clone_reader().unwrap();
            let writer = pair.master.take_writer().unwrap();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            });
            let mut s = Session { rx, raw: Vec::new(), scanner: Scanner::new(), done: Vec::new(), writer, child };
            s.wait_for_prompts(1);
            s
        }

        fn wait_for_prompts(&mut self, n: usize) {
            let deadline = Instant::now() + Duration::from_secs(20);
            while prompts(&self.raw) < n {
                assert!(Instant::now() < deadline, "no prompt number {n} in 20 s; the shell printed {:?}", String::from_utf8_lossy(&self.raw));
                match self.rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(chunk) => {
                        for e in self.scanner.feed(&chunk, Instant::now()) {
                            if let Event::Finished(f) = e {
                                self.done.push(f);
                            }
                        }
                        self.raw.extend_from_slice(&chunk);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => panic!("the shell ended before prompt {n}; it printed {:?}", String::from_utf8_lossy(&self.raw)),
                }
            }
        }

        /// Types a line and waits for the next prompt.
        fn run(&mut self, line: &str) {
            let n = prompts(&self.raw) + 1;
            self.writer.write_all(format!("{line}\n").as_bytes()).unwrap();
            self.writer.flush().unwrap();
            self.wait_for_prompts(n);
        }

        fn finish(mut self) -> Vec<Finished> {
            let _ = self.child.kill();
            self.done
        }
    }

    /// The same commands in either shell: what the scanner reports must be what happened.
    fn check_commands(shell: &Path, name: &str, user_zdotdir_setup: bool) {
        let s = Scratch::new(name);
        let home = s.0.join("home");
        let sub = home.join("sub dir é");
        fs::create_dir_all(&sub).unwrap();
        let zdot = home.join("zdot");
        if user_zdotdir_setup {
            fs::create_dir_all(&zdot).unwrap();
        }
        // your own startup files run: they leave a variable and a function behind
        let rc = "export FIXIT_RC=from-rc\nfixit_fn() { echo fn-$FIXIT_RC; }\n";
        match shell.file_name().and_then(|n| n.to_str()) {
            Some("zsh") => {
                let dir = if user_zdotdir_setup { &zdot } else { &home };
                fs::write(dir.join(".zshrc"), format!("PS1='%# '\n{rc}")).unwrap();
                fs::write(dir.join(".zshenv"), "export FIXIT_ENV=from-env\n").unwrap();
            }
            _ => {
                fs::write(home.join(".bash_profile"), "PS1='$ '\n. ~/.bashrc\n").unwrap();
                fs::write(home.join(".bashrc"), format!("{rc}export FIXIT_ENV=from-env\n")).unwrap();
            }
        }
        let mut sh = Session::start(shell, &s.0.join("shell"), &home, user_zdotdir_setup.then_some(zdot.as_path()));
        let cd = format!("cd \"{}\" && false", sub.display());
        for line in ["true", "false", "ls /nope-fixit", &cd, "fixit_fn; echo $FIXIT_ENV", "sh -c 'exit 3'", "echo 'two words' | cat"] {
            sh.run(line);
        }
        let done = sh.finish();
        assert_eq!(done.len(), 7, "{done:#?}");
        // the shell starts in your home
        assert_eq!((done[0].command.as_str(), done[0].exit), ("true", 0));
        assert_eq!(fs::canonicalize(&done[0].cwd).unwrap(), home);
        assert_eq!((done[1].command.as_str(), done[1].exit), ("false", 1));
        assert_eq!(done[2].command, "ls /nope-fixit");
        assert_ne!(done[2].exit, 0);
        assert!(done[2].output_tail.contains("nope-fixit"), "{:?}", done[2].output_tail);
        assert_eq!(done[2].cwd, done[1].cwd);
        // a cd, then a failure: the directory it happened in, spaces and accents decoded again
        assert_eq!((done[3].command.as_str(), done[3].exit), (cd.as_str(), 1));
        assert_eq!(Path::new(&done[3].cwd), sub);
        assert_eq!(done[4].output_tail, "fn-from-rc\nfrom-env");
        assert_eq!(Path::new(&done[4].cwd), sub);
        assert_eq!((done[5].command.as_str(), done[5].exit), ("sh -c 'exit 3'", 3));
        assert_eq!((done[6].command.as_str(), done[6].exit, done[6].output_tail.as_str()), ("echo 'two words' | cat", 0, "two words"));
    }

    #[test]
    fn bash_reports_each_command_with_its_status_and_directory() {
        match on_path("bash") {
            Some(bash) => check_commands(&bash, "bash", false),
            None => eprintln!("skipped: no bash on PATH"),
        }
    }

    #[test]
    fn zsh_reports_each_command_with_its_status_and_directory() {
        match on_path("zsh") {
            Some(zsh) => check_commands(&zsh, "zsh", false),
            None => eprintln!("skipped: no zsh on PATH"),
        }
    }

    #[test]
    fn zsh_finds_your_files_in_your_own_zdotdir() {
        match on_path("zsh") {
            Some(zsh) => check_commands(&zsh, "zsh-zdotdir", true),
            None => eprintln!("skipped: no zsh on PATH"),
        }
    }
}
