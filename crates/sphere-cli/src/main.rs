mod doctor;
mod render;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use render::Style;
use sphere_client::{Client, ClientKind, StartParams, Update};
use sphere_proto::api::kind;
use sphere_proto::protocol::LocalZone;
use sphere_proto::{AnswerKind, EventBody, NoteSource};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Run and watch Claude Code agents through the Sphere service.
#[derive(Parser)]
#[command(name = "sphere", version)]
struct Cli {
    /// sphered's socket (default: $XDG_RUNTIME_DIR/sphere/sphered.sock, else ~/.sphere/run/sphered.sock).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run sphered in the foreground. Extra arguments go to sphered.
    Daemon {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Start an agent on a task in its own worktree and branch.
    Run {
        repo: PathBuf,
        task: String,
        /// Work in the repository itself instead of a worktree.
        #[arg(long)]
        in_place: bool,
        /// Spending limit for this task, in dollars.
        #[arg(long)]
        budget: Option<f64>,
        #[arg(long)]
        title: Option<String>,
        /// Starting permission mode: default, acceptEdits, plan.
        #[arg(long)]
        mode: Option<String>,
        /// Do not deny force pushes and history rewrites for this task.
        #[arg(long)]
        allow_rewrites: bool,
        /// Stay and watch until the first turn finishes.
        #[arg(long)]
        follow: bool,
    },
    /// List tasks and what is waiting on you.
    Ls,
    /// Follow tasks live. With a task, replay it from the start first.
    Watch {
        task: Option<String>,
        /// Also show the agent's text as it is typed.
        #[arg(long)]
        typing: bool,
    },
    /// Answer a request: yes, change (with what to do instead), or later.
    Answer {
        request: String,
        answer: AnswerArg,
        /// For `change`: what to tell the agent. For a question: your answer.
        text: Option<String>,
        /// With yes: also save the suggested rule for next time.
        #[arg(long)]
        always: bool,
        /// With yes: replace the tool input with this JSON.
        #[arg(long)]
        input: Option<String>,
    },
    /// Send a message to a task's agent.
    Send { task: String, text: String },
    /// Interrupt the current turn.
    Interrupt { task: String },
    /// Stop a task: interrupt, terminate, kill.
    Stop { task: String },
    /// Change a task's permission mode (default, acceptEdits, plan).
    Mode { task: String, mode: String },
    /// Change a task's model.
    Model { task: String, model: String },
    /// List Claude sessions that can be brought into Sphere.
    Sessions,
    /// Move a Claude session running in a terminal (or the background) into Sphere.
    TakeOver { session: String },
    /// Resume an existing Claude session as a Sphere task.
    BringIn {
        session: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        prompt: Option<String>,
    },
    /// Tell every agent started in this repository something, like "use pnpm, never npm".
    Remember {
        /// The note. Quotes are optional.
        #[arg(required = true)]
        text: Vec<String>,
        /// Tell agents in every repository, not only this one.
        #[arg(long)]
        all: bool,
    },
    /// List the notes agents start with, by repo, with their ids.
    Memory {
        #[command(subcommand)]
        action: Option<MemoryAction>,
    },
    /// Agents on your schedule: write a routine as one line and Sphere runs it as a task.
    Protocol {
        #[command(subcommand)]
        command: ProtocolCommand,
    },
    /// Check that everything Sphere needs is in place.
    Doctor {
        /// The agent command to check.
        #[arg(long, default_value = "claude")]
        agent: String,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum MemoryAction {
    /// Remove a note: its id is the number in `sphere memory`.
    Rm { id: u64 },
}

#[derive(Subcommand)]
enum ProtocolCommand {
    /// Write a protocol: "<schedule> [<repo>]: <what to do>", e.g. "every weekday 9:00 sphere: update the dependencies".
    ///
    /// Schedules: every day 9:00, daily 9:00, every weekday 9:00, every monday 9:00, every mon,wed,fri 18:30, every hour,
    /// every 2 hours, every 30 minutes (at least 15), and once: at 18:00, tomorrow 9:00, tonight, in 2 hours, in 45 minutes.
    Add {
        /// The line, in quotes.
        text: String,
        /// The repo for a line that names none (default: the current folder).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// List the protocols, when they run next and how their last run went.
    #[command(alias = "ls")]
    List,
    /// Delete a protocol. The tasks it started stay.
    Rm { id: String },
    /// Turn a protocol on.
    On { id: String },
    /// Turn a protocol off.
    Off { id: String },
    /// Run a protocol now, besides its schedule.
    Run { id: String },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum AnswerArg {
    Yes,
    Change,
    Later,
}

fn socket_path(cli: &Cli) -> PathBuf {
    cli.socket
        .clone()
        .unwrap_or_else(sphere_proto::paths::default_socket)
}

fn style() -> Style {
    Style {
        color: unsafe { libc::isatty(1) } == 1,
    }
}

async fn connect(socket: &Path) -> Result<Client> {
    let c = Client::connect(socket).await.with_context(|| {
        format!(
            "is sphered running? (`sphere daemon`) socket: {}",
            socket.display()
        )
    })?;
    c.hello(ClientKind::Cli).await?;
    Ok(c)
}

fn print_lines(lines: Vec<String>) {
    let mut out = std::io::stdout().lock();
    for l in lines {
        let _ = writeln!(out, "{l}");
    }
    let _ = out.flush();
}

/// Print events as they come. With `until`, stop when that task's first turn
/// ends (exit code 0 if it went well).
async fn watch(
    client: &Client,
    task: Option<String>,
    from: u64,
    typing: bool,
    until: Option<String>,
) -> Result<ExitCode> {
    let st = style();
    let (_, mut sub) = client.subscribe(from, task).await?;
    while let Some(update) = sub.next().await {
        match update {
            Update::Event(e) => {
                print_lines(render::event_lines(&e, &st));
                if until.as_deref() == Some(e.task.as_str()) {
                    match &e.body {
                        EventBody::Turn {
                            ok, interrupted, ..
                        } => {
                            return Ok(if *ok || *interrupted {
                                ExitCode::SUCCESS
                            } else {
                                ExitCode::FAILURE
                            })
                        }
                        EventBody::Failed { .. } | EventBody::Stopped {} => {
                            return Ok(ExitCode::FAILURE)
                        }
                        _ => {}
                    }
                }
            }
            Update::Live(f) if typing => {
                print!("{}", f.text);
                let _ = std::io::stdout().flush();
            }
            Update::Live(_) => {}
            Update::Lagged(e) => bail!("fell behind sphered: {e}; run the command again"),
            Update::Closed => bail!("sphered closed the connection"),
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The Unix time now.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

async fn protocol(client: &Client, command: ProtocolCommand) -> Result<()> {
    let zone = LocalZone;
    match command {
        ProtocolCommand::Add { text, repo } => {
            let repo = match repo {
                Some(r) => r,
                None => std::env::current_dir().context("cannot tell which folder this is")?,
            };
            let repo = repo.to_string_lossy().to_string();
            let p = client.protocol_add(&text, Some(&repo)).await?;
            println!("{}", render::protocol_saved(&p, unix_now(), &zone));
        }
        ProtocolCommand::List => {
            let snap = client.snapshot().await?;
            print_lines(render::protocol_table(&snap, unix_now(), &zone));
        }
        ProtocolCommand::Rm { id } => {
            client.protocol_remove(&id).await?;
            println!("removed {id}");
        }
        ProtocolCommand::On { id } => {
            let p = client.protocol_set_enabled(&id, true).await?;
            println!("{}", render::protocol_saved(&p, unix_now(), &zone));
        }
        ProtocolCommand::Off { id } => {
            client.protocol_set_enabled(&id, false).await?;
            println!("{id} is off");
        }
        ProtocolCommand::Run { id } => {
            let task = client.protocol_run_now(&id).await?;
            println!("started {task}");
            println!("watch it with: sphere watch {task}");
        }
    }
    Ok(())
}

fn find_sphered() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("sphered");
            if sibling.exists() {
                return sibling;
            }
        }
    }
    PathBuf::from("sphered")
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("sphere: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let socket = socket_path(&cli);
    match cli.command {
        Command::Daemon { mut args } => {
            use std::os::unix::process::CommandExt;
            if let Some(s) = &cli.socket {
                args.splice(
                    0..0,
                    ["--socket".to_string(), s.to_string_lossy().to_string()],
                );
            }
            let err = std::process::Command::new(find_sphered()).args(args).exec();
            bail!("could not start sphered: {err}");
        }
        Command::Doctor { agent, state_dir } => {
            let dir = state_dir.unwrap_or_else(sphere_proto::paths::default_state_dir);
            let checks = doctor::run_all(&agent, &dir, &socket).await;
            println!("{}", doctor::render(&checks));
            let failed = checks.iter().any(|c| c.level == doctor::Level::Fail);
            return Ok(if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            });
        }
        _ => {}
    }
    let client = connect(&socket).await?;
    match cli.command {
        Command::Run {
            repo,
            task,
            in_place,
            budget,
            title,
            mode,
            allow_rewrites,
            follow,
        } => {
            let repo = repo
                .canonicalize()
                .with_context(|| format!("{} does not exist", repo.display()))?;
            let id = client
                .start_task(StartParams {
                    repo: repo.to_string_lossy().to_string(),
                    prompt: task,
                    title,
                    in_place,
                    budget_usd: budget,
                    mode,
                    model: None,
                    allow_rewrites,
                })
                .await?;
            println!("started {id}");
            if follow {
                return watch(&client, Some(id.clone()), 0, false, Some(id)).await;
            }
            println!("watch it with: sphere watch {id}");
        }
        Command::Ls => print_lines(render::ls_table(&client.snapshot().await?)),
        Command::Watch { task, typing } => {
            let snap = client.snapshot().await?;
            let from = if task.is_some() {
                0
            } else {
                print_lines(render::ls_table(&snap));
                let st = style();
                for r in &snap.requests {
                    print_lines(render::request_card(&r.info, &st));
                }
                snap.seq
            };
            return watch(&client, task, from, typing, None).await;
        }
        Command::Answer {
            request,
            answer,
            text,
            always,
            input,
        } => {
            let input = input
                .map(|i| serde_json::from_str(&i))
                .transpose()
                .context("--input is not JSON")?;
            client
                .answer(sphere_client::AnswerParams {
                    request: request.clone(),
                    answer: match answer {
                        AnswerArg::Yes => AnswerKind::Yes,
                        AnswerArg::Change => AnswerKind::Change,
                        AnswerArg::Later => AnswerKind::Later,
                    },
                    text,
                    input,
                    always,
                })
                .await?;
            println!("answered {request}");
        }
        Command::Send { task, text } => client.send(&task, &text).await?,
        Command::Interrupt { task } => client.interrupt(&task).await?,
        Command::Stop { task } => {
            client.stop(&task).await?;
            println!("stopped {task}");
        }
        Command::Mode { task, mode } => client.set_mode(&task, &mode).await?,
        Command::Model { task, model } => client.set_model(&task, &model).await?,
        Command::Sessions => {
            for s in client.session_list().await? {
                let running = match (&s.where_, &s.state) {
                    (Some(w), Some(st)) if s.running => format!("running in {w}, {st}"),
                    _ if s.running => "running".to_string(),
                    _ => "idle   ".to_string(),
                };
                println!(
                    "{}  {}  {}{}  {}",
                    s.session_id,
                    running,
                    s.cwd.as_deref().unwrap_or("-"),
                    s.task
                        .as_ref()
                        .map(|t| format!("  (task {t})"))
                        .unwrap_or_default(),
                    s.title.as_deref().unwrap_or("")
                );
            }
        }
        Command::TakeOver { session } => {
            let r = client.take_over(&session).await?;
            println!("Took over {} from {} as task {}", r.title, r.from, r.task);
            if r.idle {
                println!("it is waiting for your message");
            }
        }
        Command::BringIn {
            session,
            cwd,
            prompt,
        } => {
            let id = client
                .bring_in(sphere_client::BringInParams {
                    session_id: session,
                    cwd: cwd.map(|c| c.to_string_lossy().to_string()),
                    prompt,
                })
                .await?;
            println!("started {id}");
        }
        Command::Remember { text, all } => {
            let cwd = std::env::current_dir().context("cannot tell which folder you are in")?;
            let repo = (!all).then(|| cwd.to_string_lossy().to_string());
            let added = client
                .memory_add(repo.as_deref(), &text.join(" "), NoteSource::Cli)
                .await
                .map_err(|e| match e.kind() {
                    Some(kind::BAD_REPO) => anyhow::anyhow!(
                        "{} is not in a git repository: run this in one, or add --all",
                        cwd.display()
                    ),
                    _ => e.into(),
                })?;
            println!("{}", render::remembered(&added));
        }
        Command::Memory { action: None } => {
            print_lines(render::memory_table(
                &client.memory_list(None).await?,
                &style(),
            ));
        }
        Command::Memory {
            action: Some(MemoryAction::Rm { id }),
        } => {
            client.memory_remove(id).await?;
            println!("removed note {id}");
        }
        Command::Protocol { command } => protocol(&client, command).await?,
        Command::Daemon { .. } | Command::Doctor { .. } => unreachable!(),
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Command {
        let mut all = vec!["sphere"];
        all.extend(args);
        Cli::try_parse_from(all).unwrap().command
    }

    #[test]
    fn remember_takes_the_note_with_or_without_quotes() {
        let Command::Remember { text, all } = parse(&["remember", "use pnpm, never npm"]) else {
            panic!("not remember")
        };
        assert_eq!((text.join(" "), all), ("use pnpm, never npm".into(), false));
        let Command::Remember { text, all } = parse(&["remember", "--all", "use", "pnpm"]) else {
            panic!("not remember")
        };
        assert_eq!((text.join(" "), all), ("use pnpm".into(), true));
        assert!(Cli::try_parse_from(["sphere", "remember"]).is_err());
    }

    #[test]
    fn memory_lists_and_removes_by_id() {
        assert!(matches!(
            parse(&["memory"]),
            Command::Memory { action: None }
        ));
        assert!(matches!(
            parse(&["memory", "rm", "7"]),
            Command::Memory {
                action: Some(MemoryAction::Rm { id: 7 })
            }
        ));
        assert!(Cli::try_parse_from(["sphere", "memory", "rm"]).is_err());
        assert!(Cli::try_parse_from(["sphere", "memory", "rm", "x"]).is_err());
    }
}
