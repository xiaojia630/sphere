# sphered: the agent service

`sphered` is the background service behind Sphere OS. It runs real coding agents on your repos and streams what they do, so the HUD (and anything else) can show it live. Milestone 1 proves the loop end to end without the HUD: you start a task from a terminal, a real Claude Code agent works on a repo, and you watch its steps, output and permission requests stream in.

## Pieces

- `crates/sphere-proto`: the event and request types, shared by the service, the CLI and later the HUD. Serde, JSON.
- `crates/sphered`: the service. Tokio. Listens on a Unix socket, spawns one agent process per task, turns the agent's output into Sphere events, keeps task state.
- `crates/sphere-cli` (binary `sphere`): a thin client for the socket. `sphere run`, `ls`, `watch`, `send`, `approve`, `deny`, `stop`.

## Running an agent

One task is one Claude Code process in headless mode:

```
claude -p --output-format stream-json --input-format stream-json --verbose \
  --permission-prompt-tool mcp__sphere__approve --mcp-config <generated json> \
  --permission-mode acceptEdits
```

- The prompt and any follow-ups go in as stream-json user messages on stdin, so `send` can talk to a running agent.
- stdout is read line by line. Unknown line types and fields are ignored (logged at debug), never fatal.
- Working directory: by default a git worktree per task at `<repo>/.sphere/worktrees/<task-id>` on a new branch `sphere/<slug>`, so parallel agents never step on each other. `--in-place` runs in the repo itself.
- The agent command is configurable (`SPHERE_AGENT_CMD`), so tests run a fake agent that replays recorded stream-json fixtures. Tests never call the real Claude.

## Events

Every event carries `task`, a per-task `seq` (monotonic, for replay) and `at` (RFC 3339).

| kind | when | fields |
|---|---|---|
| `task_started` | process spawned | title, repo, workdir, branch |
| `plan` | the agent writes its todo list (TodoWrite) | items [{text, status}] |
| `step` | a tool call starts | id, kind (read, search, edit, run, web, delegate, plan, other), title, detail |
| `output` | tool result text, chunked | step, text, is_error |
| `step_done` | the tool result arrives | id, ok, summary |
| `file_touched` | Edit/Write/MultiEdit | path, added, removed |
| `say` | assistant prose | text |
| `needs_you` | a permission request | request, tool, input, summary |
| `resolved` | you answered it | request, decision (allow, deny), note |
| `finished` | the result line, or the process exits | ok, summary, turns, duration_ms, cost_usd |
| `error` | anything that breaks the task | message |

Step titles are short and human: `Read src/auth/session.ts`, `Search "setToken"`, `Run npx playwright test`, `Edit session.ts`. Every task also keeps a one-line `status` (for example "Running npx playwright test (step 7)") updated on each step, so "what's it doing?" can be answered instantly from state with no model call.

## Permissions (the "needs you" loop)

`--permission-prompt-tool mcp__sphere__approve` makes Claude Code ask an MCP tool before anything that needs permission. `sphered` writes an MCP config that runs `sphered mcp-permission --socket <path> --task <id>`, a tiny stdio MCP server (JSON-RPC 2.0: `initialize`, `tools/list`, `tools/call`) with one tool, `approve`. On a call it forwards the request to the service as `needs_you`, waits for `resolve`, and returns `{"behavior":"allow","updatedInput":...}` or `{"behavior":"deny","message":...}` as the tool's text result. Requests wait indefinitely; `stop` denies anything pending.

## Socket API

Unix socket at `$XDG_RUNTIME_DIR/sphere/sphered.sock`, falling back to `~/.sphere/sphered.sock`. Newline-delimited JSON. Every request has an `id`; every reply echoes it.

- `{"id":1,"op":"start","repo":"/path","prompt":"...","title":"...","in_place":false}` returns `{"id":1,"ok":true,"task":"t_..."}`
- `{"op":"list"}` returns tasks: id, title, state (working, needs_you, done, failed, stopped), status, started_at
- `{"op":"subscribe","task":"t_..." | "*","since":0}` replays from `since`, then streams events on the same connection
- `{"op":"send","task":"t_...","text":"..."}`
- `{"op":"resolve","request":"r_...","decision":"allow" | "deny","note":"..."}`
- `{"op":"stop","task":"t_..."}` (interrupt, then kill after 5 s)
- `{"op":"status"}` returns each task's one-line status

## State

In memory, plus an append-only log per task at `~/.sphere/tasks/<id>/events.jsonl`, so a restarted service and a late subscriber can replay everything.

## Later

The Rust HUD subscribes to `*` and draws Working and Agent moment from these events. Then agent-detection rules for other agents (herdr style), Docker, voice.
