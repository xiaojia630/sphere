//! Policy. Tier 1: rules in Claude Code's own syntax, compiled into each
//! agent's `--settings` so they are decided with no round trip. Tier 2: a small
//! rule engine for what needs context Claude Code lacks (the branch, say).
//! Anything neither tier settles goes to the human.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}

/// A tier-2 rule: matches a tool and a glob over its main input.
#[derive(Debug, Clone, Deserialize)]
pub struct AutoRule {
    /// Tool name, or `*`.
    pub tool: String,
    /// Glob (`*` any text, `?` one character) over the tool's main input: the
    /// command for Bash, the path for file tools, the URL for WebFetch.
    /// `{branch}` stands for the task's branch. Default: anything.
    #[serde(default)]
    pub pattern: Option<String>,
    pub decision: Decision,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SandboxSection {
    enabled: Option<bool>,
    #[serde(default)]
    allowed_domains: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RulesSection {
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
    #[serde(default)]
    ask: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    /// Honour allow rules and sandbox settings from repo policy files. Off by
    /// default: a repo you just cloned must not widen what its agent may do.
    #[serde(default)]
    trust_repos: bool,
    #[serde(default)]
    sandbox: SandboxSection,
    #[serde(default)]
    rules: RulesSection,
    #[serde(default)]
    auto: Vec<AutoRule>,
}

#[derive(Debug, Clone)]
pub struct Policy {
    pub sandbox_enabled: bool,
    pub allowed_domains: Vec<String>,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub ask: Vec<String>,
    /// Denied by default, lifted by `allow_rewrites` at task start.
    pub rewrite_deny: Vec<String>,
    pub auto: Vec<AutoRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow(String),
    Deny(String),
    Ask,
}

pub struct Ctx<'a> {
    pub branch: Option<&'a str>,
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            sandbox_enabled: true,
            allowed_domains: vec![],
            // Reads, searches, edits and tests run freely; sandboxed shell
            // commands are auto-allowed by the sandbox itself.
            allow: strs(&[
                "Bash(git status:*)",
                "Bash(git diff:*)",
                "Bash(git log:*)",
                "Bash(git show:*)",
                "Bash(git add:*)",
                "Bash(git commit:*)",
                "Bash(git branch:*)",
                "Bash(git switch:*)",
                "Bash(git stash:*)",
                "Bash(cargo test:*)",
                "Bash(cargo check:*)",
                "Bash(cargo build:*)",
                "Bash(cargo clippy:*)",
                "Bash(cargo fmt:*)",
                "Bash(npm test:*)",
                "Bash(pytest:*)",
            ]),
            deny: vec![],
            // Anything that leaves the machine always asks.
            ask: strs(&[
                "Bash(git push:*)",
                "Bash(gh:*)",
                "Bash(curl:*)",
                "Bash(wget:*)",
                "Bash(ssh:*)",
                "Bash(scp:*)",
                "Bash(rsync:*)",
                "Bash(npm publish:*)",
                "Bash(cargo publish:*)",
                "Bash(docker push:*)",
                "Bash(docker login:*)",
                "WebFetch",
            ]),
            rewrite_deny: strs(&[
                "Bash(git push --force:*)",
                "Bash(git push -f:*)",
                "Bash(git push --force-with-lease:*)",
                "Bash(git push * --force*)",
                "Bash(git push * -f)",
                "Bash(git rebase:*)",
                "Bash(git reset --hard:*)",
                "Bash(git commit --amend:*)",
                "Bash(git filter-branch:*)",
                "Bash(git filter-repo:*)",
            ]),
            auto: vec![],
        }
    }
}

fn read_file(path: &Path) -> Result<Option<PolicyFile>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

fn push_unique(into: &mut Vec<String>, from: Vec<String>) {
    for r in from {
        if !into.contains(&r) {
            into.push(r);
        }
    }
}

impl Policy {
    /// The defaults, then the user's file, then the repo's.
    pub fn load(user_file: &Path, repo_file: Option<&Path>) -> Result<Policy> {
        let mut p = Policy::default();
        let user = read_file(user_file)?;
        let trust_repos = user.as_ref().map(|u| u.trust_repos).unwrap_or(false);
        if let Some(u) = user {
            p.merge(u, true);
        }
        if let Some(repo) = repo_file.map(read_file).transpose()?.flatten() {
            p.merge(repo, trust_repos);
        }
        Ok(p)
    }

    fn merge(&mut self, f: PolicyFile, trusted: bool) {
        if trusted {
            if let Some(on) = f.sandbox.enabled {
                self.sandbox_enabled = on;
            }
            push_unique(&mut self.allowed_domains, f.sandbox.allowed_domains);
            push_unique(&mut self.allow, f.rules.allow);
            self.auto.extend(f.auto);
        } else {
            // Untrusted: may only add restrictions.
            self.auto.extend(f.auto.into_iter().filter(|r| r.decision != Decision::Allow));
        }
        push_unique(&mut self.deny, f.rules.deny);
        push_unique(&mut self.ask, f.rules.ask);
    }

    /// The `--settings` JSON for an agent.
    pub fn settings_json(&self, allow_rewrites: bool) -> Value {
        let mut deny = self.deny.clone();
        let mut ask = self.ask.clone();
        if !allow_rewrites {
            push_unique(&mut deny, self.rewrite_deny.clone());
        }
        // A rule that denies must not also ask.
        ask.retain(|r| !deny.contains(r));
        let mut sandbox = json!({
            "enabled": self.sandbox_enabled,
            "autoAllowBashIfSandboxed": true,
        });
        if !self.allowed_domains.is_empty() {
            sandbox["network"] = json!({ "allowedDomains": self.allowed_domains });
        }
        json!({
            "permissions": { "allow": self.allow, "deny": deny, "ask": ask },
            "sandbox": sandbox,
        })
    }

    /// Tier 2. Deny beats ask beats allow; no match means ask the human.
    pub fn decide(&self, tool: &str, input: &Value, ctx: &Ctx) -> Verdict {
        let main = main_input(tool, input);
        let mut best: Option<(Decision, String)> = None;
        for rule in &self.auto {
            if rule.tool != "*" && rule.tool != tool {
                continue;
            }
            if let Some(pat) = &rule.pattern {
                let pat = match (pat.contains("{branch}"), ctx.branch) {
                    (true, Some(b)) => pat.replace("{branch}", b),
                    (true, None) => continue,
                    (false, _) => pat.clone(),
                };
                if !glob_match(&pat, &main) {
                    continue;
                }
            }
            let name = rule.name.clone().unwrap_or_else(|| {
                format!("{}({})", rule.tool, rule.pattern.as_deref().unwrap_or("*"))
            });
            let rank = |d: Decision| match d {
                Decision::Deny => 2,
                Decision::Ask => 1,
                Decision::Allow => 0,
            };
            if best.as_ref().map(|(d, _)| rank(rule.decision) > rank(*d)).unwrap_or(true) {
                best = Some((rule.decision, name));
            }
        }
        match best {
            Some((Decision::Allow, n)) => Verdict::Allow(n),
            Some((Decision::Deny, n)) => Verdict::Deny(n),
            _ => Verdict::Ask,
        }
    }
}

/// The part of a tool's input rules match against.
pub fn main_input(tool: &str, input: &Value) -> String {
    let field = |k: &str| input.get(k).and_then(|v| v.as_str()).map(str::to_string);
    match tool {
        "Bash" => field("command"),
        "Read" | "Write" | "Edit" | "MultiEdit" => field("file_path"),
        "NotebookEdit" => field("notebook_path"),
        "Glob" | "Grep" => field("pattern"),
        "WebFetch" => field("url"),
        "WebSearch" => field("query"),
        _ => None,
    }
    .unwrap_or_else(|| input.to_string())
}

/// `*` matches any run of characters (spaces included), `?` one character.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// The allow rule a permission suggestion stands for, in rule syntax.
pub fn rule_from_suggestion(s: &Value) -> Option<String> {
    if s.get("type")?.as_str()? != "addRules" || s.get("behavior").and_then(|b| b.as_str()) != Some("allow") {
        return None;
    }
    let rule = s.get("rules")?.as_array()?.first()?;
    let tool = rule.get("toolName")?.as_str()?;
    Some(match rule.get("ruleContent").and_then(|c| c.as_str()) {
        Some(c) if !c.is_empty() => format!("{tool}({c})"),
        _ => tool.to_string(),
    })
}

/// The suggestion as handed back to Claude Code: for this session only, so
/// nothing is written into the worktree's settings files.
pub fn session_scoped(s: &Value) -> Value {
    let mut s = s.clone();
    if let Some(o) = s.as_object_mut() {
        o.insert("destination".into(), json!("session"));
    }
    s
}

/// Add `rule` to the `[rules] allow` list in Sphere's policy file, keeping the
/// comments and layout of everything else.
pub fn append_allow_rule(path: &Path, rule: &str) -> Result<()> {
    use toml_edit::{value, Array, DocumentMut, Item, Table};
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let mut doc: DocumentMut = text.parse().with_context(|| format!("parse {}", path.display()))?;
    let rules = doc
        .entry("rules")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .context("`rules` is not a table")?;
    let allow = rules
        .entry("allow")
        .or_insert_with(|| value(Array::new()))
        .as_array_mut()
        .context("`rules.allow` is not an array")?;
    if !allow.iter().any(|v| v.as_str() == Some(rule)) {
        allow.push(rule);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn globs() {
        assert!(glob_match("git push * sphere/*", "git push origin sphere/fix-x"));
        assert!(glob_match("git push*", "git push"));
        assert!(!glob_match("git push * sphere/*", "git push origin main"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", ""));
        assert!(glob_match("*x*", "abxcd"));
    }

    #[test]
    fn defaults_ask_for_outbound_and_deny_rewrites() {
        let s = Policy::default().settings_json(false);
        let ask = s["permissions"]["ask"].as_array().unwrap();
        let deny = s["permissions"]["deny"].as_array().unwrap();
        assert!(ask.iter().any(|r| r == "Bash(git push:*)"));
        assert!(ask.iter().any(|r| r == "Bash(gh:*)"));
        assert!(ask.iter().any(|r| r == "Bash(curl:*)"));
        assert!(deny.iter().any(|r| r == "Bash(git push --force:*)"));
        assert!(deny.iter().any(|r| r == "Bash(git reset --hard:*)"));
        assert_eq!(s["sandbox"]["enabled"], true);
        assert_eq!(s["sandbox"]["autoAllowBashIfSandboxed"], true);
        assert!(s["sandbox"].get("network").is_none());
    }

    #[test]
    fn allow_rewrites_lifts_the_denies_but_push_still_asks() {
        let s = Policy::default().settings_json(true);
        assert!(s["permissions"]["deny"].as_array().unwrap().is_empty());
        assert!(s["permissions"]["ask"].as_array().unwrap().iter().any(|r| r == "Bash(git push:*)"));
    }

    #[test]
    fn user_file_changes_sandbox_and_adds_rules() {
        let dir = tempfile::tempdir().unwrap();
        let user = write(
            dir.path(),
            "policy.toml",
            r#"
[sandbox]
enabled = false
allowed_domains = ["crates.io"]
[rules]
allow = ["Bash(make:*)"]
deny = ["Bash(rm -rf:*)"]
"#,
        );
        let p = Policy::load(&user, None).unwrap();
        let s = p.settings_json(false);
        assert_eq!(s["sandbox"]["enabled"], false);
        assert_eq!(s["sandbox"]["network"]["allowedDomains"][0], "crates.io");
        assert!(s["permissions"]["allow"].as_array().unwrap().iter().any(|r| r == "Bash(make:*)"));
        assert!(s["permissions"]["deny"].as_array().unwrap().iter().any(|r| r == "Bash(rm -rf:*)"));
    }

    #[test]
    fn a_repo_policy_can_restrict_but_not_widen_unless_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("none.toml");
        let repo = write(
            dir.path(),
            "repo.toml",
            r#"
[sandbox]
enabled = false
[rules]
allow = ["Bash(curl:*)"]
ask = ["Bash(make deploy:*)"]
[[auto]]
tool = "Bash"
pattern = "*"
decision = "allow"
"#,
        );
        let p = Policy::load(&user, Some(&repo)).unwrap();
        let s = p.settings_json(false);
        assert_eq!(s["sandbox"]["enabled"], true, "repo cannot turn the sandbox off");
        assert!(!s["permissions"]["allow"].as_array().unwrap().iter().any(|r| r == "Bash(curl:*)"));
        assert!(s["permissions"]["ask"].as_array().unwrap().iter().any(|r| r == "Bash(make deploy:*)"));
        assert_eq!(p.decide("Bash", &json!({"command": "ls"}), &Ctx { branch: None }), Verdict::Ask);

        let user = write(dir.path(), "user.toml", "trust_repos = true\n");
        let p = Policy::load(&user, Some(&repo)).unwrap();
        assert_eq!(p.settings_json(false)["sandbox"]["enabled"], false);
        assert!(matches!(p.decide("Bash", &json!({"command": "ls"}), &Ctx { branch: None }), Verdict::Allow(_)));
    }

    #[test]
    fn typos_in_policy_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let user = write(dir.path(), "policy.toml", "[rules]\nalow = []\n");
        assert!(Policy::load(&user, None).is_err());
    }

    fn auto(tool: &str, pattern: Option<&str>, d: Decision) -> AutoRule {
        AutoRule { tool: tool.into(), pattern: pattern.map(str::to_string), decision: d, name: None }
    }

    #[test]
    fn tier2_allows_a_push_only_to_the_task_branch() {
        let mut p = Policy::default();
        p.auto.push(auto("Bash", Some("git push * {branch}"), Decision::Allow));
        let ctx = Ctx { branch: Some("sphere/fix-login") };
        let push = |b: &str| json!({"command": format!("git push origin {b}")});
        assert!(matches!(p.decide("Bash", &push("sphere/fix-login"), &ctx), Verdict::Allow(_)));
        assert_eq!(p.decide("Bash", &push("main"), &ctx), Verdict::Ask);
        assert_eq!(p.decide("Edit", &push("sphere/fix-login"), &ctx), Verdict::Ask);
        // No branch (in place): a rule that needs one never matches.
        assert_eq!(p.decide("Bash", &push("sphere/fix-login"), &Ctx { branch: None }), Verdict::Ask);
    }

    #[test]
    fn tier2_precedence_is_deny_then_ask_then_allow() {
        let mut p = Policy::default();
        p.auto.push(auto("Bash", Some("git push*"), Decision::Allow));
        p.auto.push(auto("*", Some("*prod*"), Decision::Deny));
        let ctx = Ctx { branch: None };
        assert!(matches!(p.decide("Bash", &json!({"command": "git push origin x"}), &ctx), Verdict::Allow(_)));
        assert!(matches!(p.decide("Bash", &json!({"command": "git push prod x"}), &ctx), Verdict::Deny(_)));
        p.auto.push(auto("Bash", None, Decision::Ask));
        assert_eq!(p.decide("Bash", &json!({"command": "git push origin x"}), &ctx), Verdict::Ask);
    }

    #[test]
    fn main_input_picks_the_right_field() {
        assert_eq!(main_input("Bash", &json!({"command": "ls"})), "ls");
        assert_eq!(main_input("Edit", &json!({"file_path": "/a/b"})), "/a/b");
        assert_eq!(main_input("WebFetch", &json!({"url": "https://x"})), "https://x");
        assert_eq!(main_input("Other", &json!({"a": 1})), "{\"a\":1}");
    }

    #[test]
    fn suggestions_become_rules_and_are_scoped_to_the_session() {
        let s = json!({"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"cargo test:*"}],"behavior":"allow","destination":"localSettings"});
        assert_eq!(rule_from_suggestion(&s).as_deref(), Some("Bash(cargo test:*)"));
        assert_eq!(session_scoped(&s)["destination"], "session");
        assert_eq!(s["destination"], "localSettings");
        let bare = json!({"type":"addRules","rules":[{"toolName":"WebFetch"}],"behavior":"allow"});
        assert_eq!(rule_from_suggestion(&bare).as_deref(), Some("WebFetch"));
        assert_eq!(rule_from_suggestion(&json!({"type":"setMode","mode":"acceptEdits"})), None);
        assert_eq!(
            rule_from_suggestion(&json!({"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"x"}],"behavior":"deny"})),
            None
        );
    }

    #[test]
    fn appending_a_rule_keeps_comments_and_does_not_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.toml");
        std::fs::write(&path, "# my policy\n[rules]\n# tests are fine\nallow = [\"Bash(make:*)\"]\n").unwrap();
        append_allow_rule(&path, "Bash(cargo test:*)").unwrap();
        append_allow_rule(&path, "Bash(cargo test:*)").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my policy") && text.contains("# tests are fine"), "{text}");
        assert_eq!(text.matches("cargo test").count(), 1);
        let p = Policy::load(&path, None).unwrap();
        assert!(p.allow.contains(&"Bash(make:*)".to_string()));
        assert!(p.allow.contains(&"Bash(cargo test:*)".to_string()));

        let fresh = dir.path().join("sub/new.toml");
        append_allow_rule(&fresh, "WebFetch").unwrap();
        assert!(Policy::load(&fresh, None).unwrap().allow.contains(&"WebFetch".to_string()));
    }
}
