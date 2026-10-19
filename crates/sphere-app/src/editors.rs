//! Editors the Code and Files apps hand a file to: Helix in a Sphere terminal tab (quick edits), Zed in its own
//! window (real editing). Nothing is installed from here: a missing editor says how to install it.
use std::path::{Path, PathBuf};

pub const HELIX_HINT: &str = "Helix isn't installed: brew install helix";
pub const ZED_HINT: &str = "Zed isn't installed: brew install --cask zed";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default()
}

/// Where `hx` may be: every PATH folder, then Homebrew's two.
pub fn helix_candidates(path: &[PathBuf]) -> Vec<PathBuf> {
    path.iter().map(|d| d.join("hx")).chain(["/opt/homebrew/bin/hx", "/usr/local/bin/hx"].map(PathBuf::from)).collect()
}

/// Where Zed's command line may be: the app bundles (per-user, then system, stable then preview), then `zed` on PATH.
pub fn zed_candidates(home: &Path, path: &[PathBuf]) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for base in [home.join("Applications"), PathBuf::from("/Applications")] {
        for app in ["Zed.app", "Zed Preview.app"] {
            v.push(base.join(app).join("Contents/MacOS/cli"));
        }
    }
    v.extend(["/usr/local/bin/zed", "/opt/homebrew/bin/zed"].map(PathBuf::from));
    v.extend(path.iter().map(|d| d.join("zed")));
    v
}

/// The first candidate that exists, else the install hint.
pub fn first_existing(cands: &[PathBuf], exists: &dyn Fn(&Path) -> bool, hint: &str) -> Result<PathBuf, String> {
    cands.iter().find(|p| exists(p)).cloned().ok_or_else(|| hint.to_string())
}

fn is_file(p: &Path) -> bool {
    p.is_file()
}

pub fn find_helix() -> Result<PathBuf, String> {
    first_existing(&helix_candidates(&path_dirs()), &is_file, HELIX_HINT)
}

pub fn find_zed() -> Result<PathBuf, String> {
    first_existing(&zed_candidates(&home(), &path_dirs()), &is_file, ZED_HINT)
}

/// Opens `target` (a file or a folder) in Zed's own window, at `line` for a file.
pub fn open_in_zed(target: &Path, line: Option<u32>) -> Result<(), String> {
    let zed = find_zed()?;
    let arg = match line {
        Some(l) => format!("{}:{l}", target.display()),
        None => target.display().to_string(),
    };
    std::process::Command::new(zed).arg(arg).spawn().map(|_| ()).map_err(|e| format!("Zed: {e}"))
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The shell line a terminal tab runs for a quick edit: the shell becomes Helix, so the tab closes when you quit it.
pub fn helix_line(hx: &Path, dir: &Path, file: &Path, line: Option<u32>) -> String {
    let target = match line {
        Some(l) => format!("{}:{l}", file.display()),
        None => file.display().to_string(),
    };
    format!("cd {} && exec {} {}\n", quote(&dir.display().to_string()), quote(&hx.display().to_string()), quote(&target))
}

/// Opens a terminal tab running Helix on `file` (at `line`) and gives it the keyboard. The caller closes its app so
/// the terminal shows; when Helix quits the tab closes and app `back` shows again.
pub fn edit_in_helix(terms: &mut crate::term::Terms, focus: &mut crate::panel::Focus, file: &Path, line: Option<u32>, back: usize) -> Result<(), String> {
    let hx = find_helix()?;
    let dir = file.parent().filter(|d| d.is_dir()).unwrap_or(Path::new("/"));
    let before = terms.tabs.len();
    terms.open_tab();
    // no new tab (sphere-host unreachable): never `exec` Helix in one of your shells
    if terms.tabs.len() == before {
        return Err("Couldn't open a terminal for Helix".into());
    }
    terms.active = terms.tabs.len() - 1;
    terms.version += 1;
    let t = terms.tabs.last_mut().expect("tab just opened");
    t.back_to = Some(back);
    t.send(helix_line(&hx, dir, file, line).as_bytes());
    focus.terminal = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_editor_gives_the_install_hint() {
        let none = |_: &Path| false;
        assert_eq!(first_existing(&helix_candidates(&[PathBuf::from("/x")]), &none, HELIX_HINT).unwrap_err(), "Helix isn't installed: brew install helix");
        assert_eq!(first_existing(&zed_candidates(Path::new("/h"), &[]), &none, ZED_HINT).unwrap_err(), "Zed isn't installed: brew install --cask zed");
    }

    #[test]
    fn the_search_lists_cover_the_usual_places() {
        let hx = helix_candidates(&[PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(hx[..2], [PathBuf::from("/a/hx"), PathBuf::from("/b/hx")]);
        assert!(hx.contains(&PathBuf::from("/opt/homebrew/bin/hx")) && hx.contains(&PathBuf::from("/usr/local/bin/hx")));
        let z = zed_candidates(Path::new("/Users/me"), &[PathBuf::from("/a")]);
        assert_eq!(z[0], PathBuf::from("/Users/me/Applications/Zed.app/Contents/MacOS/cli"));
        assert!(z.contains(&PathBuf::from("/Applications/Zed Preview.app/Contents/MacOS/cli")));
        assert_eq!(z.last(), Some(&PathBuf::from("/a/zed")));
    }

    #[test]
    fn the_first_hit_wins() {
        let cands = vec![PathBuf::from("/a/hx"), PathBuf::from("/b/hx")];
        let only_b = |p: &Path| p == Path::new("/b/hx");
        assert_eq!(first_existing(&cands, &only_b, HELIX_HINT), Ok(PathBuf::from("/b/hx")));
    }

    #[test]
    fn the_helix_line_quotes_paths_and_replaces_the_shell() {
        let l = helix_line(Path::new("/opt/homebrew/bin/hx"), Path::new("/p/it's"), Path::new("/p/it's/a b.rs"), Some(12));
        assert_eq!(l, "cd '/p/it'\\''s' && exec '/opt/homebrew/bin/hx' '/p/it'\\''s/a b.rs:12'\n");
        assert!(helix_line(Path::new("hx"), Path::new("/p"), Path::new("/p/a"), None).ends_with("'/p/a'\n"));
    }
}
