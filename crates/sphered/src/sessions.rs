//! Claude sessions that exist outside Sphere: running ones from
//! `claude agents --json`, finished ones from the transcripts on disk.

use crate::config::Config;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use sphere_proto::api::SessionInfo;
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// States that mean a session is still alive.
const LIVE_STATES: &[&str] = &[
    "running",
    "active",
    "working",
    "busy",
    "starting",
    "needs_input",
    "blocked",
    "waiting",
];

fn pid_alive(pid: u32) -> bool {
    pid > 0 && unsafe { libc::kill(pid as i32, 0) } == 0
}

/// Entries of `claude agents --json`. Tolerates an array or an object holding
/// one, and either naming style for the fields.
pub fn parse_agents_json(text: &str) -> Result<Vec<SessionInfo>> {
    let v: Value = serde_json::from_str(text).context("`agents --json` did not print JSON")?;
    let list = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o
            .get("sessions")
            .or_else(|| o.get("agents"))
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => vec![],
    };
    let get = |e: &Value, keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|k| e.get(k).and_then(|x| x.as_str()).map(str::to_string))
    };
    Ok(list
        .iter()
        .filter_map(|e| {
            let session_id = get(e, &["sessionId", "session_id", "id"])?;
            let pid = e
                .get("pid")
                .and_then(|p| p.as_u64())
                .map(|p| p as u32)
                .filter(|p| *p > 0);
            let state = get(e, &["state", "status"])
                .unwrap_or_default()
                .to_ascii_lowercase();
            Some(SessionInfo {
                session_id,
                cwd: get(e, &["cwd", "workingDirectory"]),
                title: get(e, &["name", "intent", "title", "summary"]),
                updated_at: get(e, &["updatedAt", "updated_at"]),
                running: LIVE_STATES.contains(&state.as_str()) || pid.is_some_and(pid_alive),
                pid,
                task: None,
            })
        })
        .collect())
}

fn rfc3339(t: SystemTime) -> Option<String> {
    let ms = t.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis() as u64;
    Some(sphere_proto::event::rfc3339_from_unix_ms(ms))
}

/// The session summarised from the head of a transcript file.
fn read_transcript(path: &Path) -> Option<SessionInfo> {
    use std::io::BufRead;
    let session_id = path.file_stem()?.to_string_lossy().to_string();
    let file = std::fs::File::open(path).ok()?;
    let mut cwd = None;
    let mut title = None;
    for line in std::io::BufReader::new(file)
        .lines()
        .take(60)
        .map_while(|l| l.ok())
    {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if cwd.is_none() {
            cwd = v.get("cwd").and_then(|c| c.as_str()).map(str::to_string);
        }
        if title.is_none() && v.get("type").and_then(|t| t.as_str()) == Some("user") {
            let c = v.pointer("/message/content");
            let text = match c {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Array(a)) => a
                    .iter()
                    .find_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .map(str::to_string),
                _ => None,
            };
            title = text.map(|t| t.lines().next().unwrap_or("").chars().take(100).collect());
        }
        if cwd.is_some() && title.is_some() {
            break;
        }
    }
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    Some(SessionInfo {
        session_id,
        cwd,
        title,
        updated_at: modified.and_then(rfc3339),
        running: false,
        pid: None,
        task: None,
    })
}

pub fn scan_transcripts(projects: &Path) -> Vec<SessionInfo> {
    let mut out = vec![];
    let Ok(dirs) = std::fs::read_dir(projects) else {
        return out;
    };
    for dir in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().is_some_and(|e| e == "jsonl") {
                out.extend(read_transcript(&p));
            }
        }
    }
    out
}

async fn run_json(cmd: &[String]) -> Result<String> {
    let Some((prog, args)) = cmd.split_first() else {
        bail!("empty command")
    };
    let fut = tokio::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(15), fut)
        .await
        .with_context(|| format!("`{}` timed out", cmd.join(" ")))?
        .with_context(|| format!("run `{}`", cmd.join(" ")))?;
    if !out.status.success() {
        bail!(
            "`{}` failed: {}",
            cmd.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Every session we know of, newest first. The agents list wins over the
/// transcript of the same session because it knows whether it is running.
/// `known` maps session ids already owned by a Sphere task to that task.
pub async fn list(cfg: &Config, known: &HashMap<String, String>) -> Result<Vec<SessionInfo>> {
    let mut by_id: HashMap<String, SessionInfo> = HashMap::new();
    for s in scan_transcripts(&cfg.projects_dir()) {
        by_id.insert(s.session_id.clone(), s);
    }
    match run_json(&cfg.session_list_cmd())
        .await
        .and_then(|t| parse_agents_json(&t))
    {
        Ok(agents) => {
            for a in agents {
                let merged = match by_id.remove(&a.session_id) {
                    Some(t) => SessionInfo {
                        cwd: a.cwd.clone().or(t.cwd),
                        title: a.title.clone().or(t.title),
                        updated_at: a.updated_at.clone().or(t.updated_at),
                        ..a
                    },
                    None => a,
                };
                by_id.insert(merged.session_id.clone(), merged);
            }
        }
        Err(e) => tracing::debug!("could not list running sessions: {e:#}"),
    }
    let mut out: Vec<SessionInfo> = by_id.into_values().collect();
    for s in &mut out {
        s.task = known.get(&s.session_id).cloned();
    }
    out.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then(a.session_id.cmp(&b.session_id))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_agents_json_in_both_shapes() {
        let arr = r#"[{"sessionId":"s1","cwd":"/r","state":"running","name":"fix"},{"session_id":"s2","pid":0,"state":"stopped"},{"nope":1}]"#;
        let l = parse_agents_json(arr).unwrap();
        assert_eq!(l.len(), 2);
        assert!(l[0].running && l[0].title.as_deref() == Some("fix"));
        assert!(!l[1].running && l[1].pid.is_none());

        let obj = r#"{"sessions":[{"id":"s3","state":"idle","pid":1}]}"#;
        let l = parse_agents_json(obj).unwrap();
        assert_eq!(l[0].session_id, "s3");
        assert!(parse_agents_json("{}").unwrap().is_empty());
        assert!(parse_agents_json("garbage").is_err());
    }

    #[test]
    fn a_live_pid_counts_as_running() {
        let me = std::process::id();
        let l = parse_agents_json(&format!(r#"[{{"sessionId":"s","pid":{me}}}]"#)).unwrap();
        assert!(l[0].running);
    }

    #[test]
    fn scans_transcripts_for_cwd_and_first_message() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("-home-me-repo");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("abc-123.jsonl"),
            concat!(
                r#"{"type":"summary","summary":"x"}"#, "\n",
                r#"{"type":"user","cwd":"/home/me/repo","message":{"role":"user","content":"Fix the flaky test\nplease"}}"#, "\n",
                r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}}"#, "\n",
            ),
        )
        .unwrap();
        std::fs::write(proj.join("notes.txt"), "ignored").unwrap();
        let l = scan_transcripts(tmp.path());
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].session_id, "abc-123");
        assert_eq!(l[0].cwd.as_deref(), Some("/home/me/repo"));
        assert_eq!(l[0].title.as_deref(), Some("Fix the flaky test"));
        assert!(l[0].updated_at.as_deref().unwrap().ends_with('Z'));
        assert!(scan_transcripts(&tmp.path().join("missing")).is_empty());
    }
}
