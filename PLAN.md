# Blue — Plan

Voice-driven control of Claude Code running on my own machines.

I talk to a LiveKit voice agent. The agent talks to a central management **server**.
The server routes work to a **daemon** on whichever machine should do it. The daemon drives the
Claude Agent SDK through a thin TypeScript **bridge**, which runs the local `claude` binary.

This replicates the loop T3 Code uses (host process → `@anthropic-ai/claude-agent-sdk`
`query()` → `claude` binary over stream-json), but with the host split into a cloud server and
per-machine daemons that we own, and with voice as the frontend.

---

## 1. Goals and non-goals

**Goals**

- Speak a task ("on my desktop, in the blue repo, add a health endpoint") and have Claude Code
  do it on that machine.
- Hear back when work finishes, with a short spoken summary; ask "what's it doing?" any time.
- Follow up and switch between sessions and machines by voice.
- Multiple machines, each connecting outbound to the server (no inbound ports, works behind NAT).
- Sessions survive server restarts, daemon restarts, and network drops.
- Server is a Helm chart deployable to a self-run Kubernetes cluster, backed by external Postgres.

**Non-goals (for now)**

- Multi-user / multi-tenant. Single user (me), though the schema keeps a `users` table.
- Permission prompts. Claude runs with `bypassPermissions`; the protocol reserves room to add
  approval later.
- A custom web UI. LiveKit's Agents Playground is the frontend until phase 7.
- Horizontal scaling of the server. One replica; see §11 for the path to more.

---

## 2. Languages: who owns what

| Component | Language | Why |
|---|---|---|
| `voice` — LiveKit agent | **Python** | `livekit-agents` (Python), hosted on LiveKit Cloud |
| `server` — management server | **Rust** | ours; deployed to k8s via Helm |
| `daemon` — local client | **Rust** | ours; all content flows through it |
| `claude-bridge` — SDK adapter | **TypeScript** | the minimum needed to use `@anthropic-ai/claude-agent-sdk` |
| `cli` — text client | **Rust** | drive the system without voice; admin tasks |

The bridge has no business logic: no network, no persistence, no decisions. It is a
stdio adapter around `query()`.

**Wire format: Protocol Buffers everywhere we control both ends.** All `.proto` files live in
`proto/` and are the single source of truth:

| Link | Transport | Encoding |
|---|---|---|
| bridge ⇄ daemon | child-process stdin/stdout | length-delimited protobuf frames (varint length + message) |
| daemon ⇄ server | gRPC over HTTP/2 + TLS (`DaemonService.Attach`, bidi stream) | protobuf |
| voice ⇄ server | gRPC (`ClientService`, `AuthService`) | protobuf |
| cli ⇄ server | gRPC (same services) | protobuf |

The only JSON in the system is content we don't own: the Agent SDK's `SDKMessage` objects,
which ride inside protobuf as an opaque `json` string field (§5.2).

Codegen: Rust via `tonic-prost-build` (with a vendored `protoc`, no install needed), TypeScript
via `@bufbuild/protoc-gen-es` (driven by `buf generate`), Python via `grpcio-tools`. `buf lint`
enforces style; `buf breaking` guards compatibility once v1 ships.

---

## 3. Architecture

```
  Browser / phone (Agents Playground, later apps/web)
        │ WebRTC                                 ▲ GitHub sign-in, room token
        ▼                                        │
  ┌─────────────── LiveKit Cloud ───────────┐    │
  │ SFU  ◀──▶  voice (Python agent job)     │    │
  └──────────────────┬──────────────────────┘    │
                     │ gRPC ClientService (protobuf, service token)
                     ▼                           │
  ┌──────────── self-run k8s (Helm: blue-server) ┴──────────┐
  │  server (Rust, tonic gRPC)                              │
  │  auth · machine registry · sessions/turns · routing ·   │
  │  event log + fan-out · LiveKit token minting            │
  │            │ sqlx                                       │
  └────────────┼────────────────────────────────────────────┘
               ▼
         external Postgres (database `blue`, role `blue_server`)
               ▲
               │ gRPC DaemonService.Attach bidi stream (outbound, machine token)
  ┌────────────┴───────────── each machine ─────────────────┐
  │ blue-daemon (Rust)                                       │
  │  server connection · session supervisor · SDK→event      │
  │  translation · SQLite outbox                             │
  │       │ stdin/stdout length-delimited protobuf           │
  │       │ (one child per session)                          │
  │       ▼                                                  │
  │ claude-bridge (TS, bun-compiled exe)                     │
  │       │ @anthropic-ai/claude-agent-sdk query()           │
  │       ▼                                                  │
  │ claude (installed binary, uses the machine's login)      │
  └──────────────────────────────────────────────────────────┘
```

### A voice turn, end to end

1. I say "in the blue repo on my desktop, add a health endpoint."
2. Voice LLM calls `start_task(machine="desktop", title="Health endpoint", instructions="in the
   blue repo, add a health endpoint")`.
3. Voice sends `session.create` to the server; server persists session + turn, routes
   `session.start` to the desktop daemon; voice immediately says "On it."
4. Daemon spawns a bridge, sends `start` + `prompt`. Bridge calls `query()`.
5. SDK messages stream bridge → daemon (raw). Daemon translates to events, writes them to its
   outbox, sends them to the server; server stores them and fans out to subscribers.
6. Voice stays quiet during progress. If I ask "what's it doing?", it calls `get_status`, which
   returns recent progress events, and summarizes.
7. On `turn.completed`, voice waits for a quiet moment and speaks a 1–2 sentence summary.
8. "Also add a test for it" → `send_follow_up` → same session, same bridge process, next turn.

---

## 4. Repository layout

Monorepo at `~/documents/code/blue`.

```
blue/
├── PLAN.md
├── README.md
├── justfile                   # cross-language task runner (dev, codegen, build, test)
├── Cargo.toml                 # Rust workspace
├── rust-toolchain.toml
├── package.json               # bun workspace root (bridge) + buf tooling
├── buf.yaml, buf.gen.yaml     # proto lint/breaking config + TS codegen
├── proto/                     # ★ single source of truth for every wire protocol
│   └── blue/
│       ├── events/v1/events.proto   # SessionEvent (translated from SDK messages)
│       ├── bridge/v1/bridge.proto   # daemon ⇄ bridge stdio frames
│       ├── daemon/v1/daemon.proto   # DaemonService (server ⇄ daemon)
│       ├── client/v1/client.proto   # ClientService (server ⇄ voice/cli)
│       └── auth/v1/auth.proto       # AuthService (GitHub login, LiveKit tokens, pairing)
├── crates/
│   ├── protocol/              # generated Rust (tonic/prost, build.rs) + framing helpers
│   │   └── src/{lib.rs, framing.rs}
│   ├── server/                # bin: blue-server
│   │   ├── src/{main.rs, config.rs, grpc/, auth/, db/, routing/, livekit.rs}
│   │   ├── migrations/        # sqlx migrations (embedded)
│   │   └── Dockerfile
│   ├── daemon/                # bin: blue-daemon (+ lib: sessions, used by tests and daemon-test)
│   │   ├── src/{main.rs, lib.rs, config.rs, server_conn.rs, supervisor.rs, session.rs,
│   │   │        bridge_proc.rs, translate.rs, recorder.rs, outbox.rs, service.rs, doctor.rs}
│   │   └── tests/{live.rs, replay.rs, translate_fixtures.rs, fixtures/, snapshots/}
│   ├── daemon-test/           # bin: blue-daemon-test (local testing tool, not shipped)
│   └── cli/                   # bin: blue
├── bridge/                    # TypeScript: claude-bridge
│   ├── package.json, tsconfig.json
│   ├── src/{main.ts, session.ts, scout.ts, replay.ts, io.ts, queue.ts, sdk.ts}
│   └── src/gen/               # generated by protoc-gen-es (committed)
├── voice/                     # Python: LiveKit agent
│   ├── pyproject.toml, uv.lock
│   ├── Dockerfile
│   ├── src/blue_voice/{agent.py, tools.py, server_client.py, notifier.py, prompts.py}
│   ├── src/blue/              # generated by grpcio-tools (committed; needed for LiveKit Cloud builds)
│   └── tests/
├── deploy/
│   ├── helm/blue-server/      # the Helm chart (§9)
│   ├── db/bootstrap.sql       # manual alternative to the bootstrap hook
│   └── compose/compose.yaml   # local Postgres (+ optional server container)
└── .github/workflows/         # §12
```

---

## 5. Protocols

Every protocol is a set of `.proto` files under `proto/blue/<area>/v1/`. Package names follow
the directory (`blue.daemon.v1`, …); `buf lint` (STANDARD rules) runs in CI.

| Package | File | Used by |
|---|---|---|
| `blue.events.v1` | `events.proto` | `SessionEvent` + `SessionState` shared by daemon, server, clients |
| `blue.bridge.v1` | `bridge.proto` | daemon ⇄ bridge stdio |
| `blue.daemon.v1` | `daemon.proto` | `DaemonService` (server ⇄ daemon) |
| `blue.client.v1` | `client.proto` | `ClientService` (server ⇄ voice, cli, later web) |
| `blue.auth.v1` | `auth.proto` | `AuthService` (GitHub login, LiveKit tokens, pairing, revocation) |

Generated code:

| Language | Tool | Output | Committed? |
|---|---|---|---|
| Rust | `tonic-prost-build` in `crates/protocol/build.rs`, `protoc-bin-vendored` | `OUT_DIR` | no (built every time) |
| TypeScript | `buf generate` + `protoc-gen-es` (only `blue/bridge`) | `bridge/src/gen/` | yes |
| Python | `grpcio-tools` (`events`, `client`, `auth`) | `voice/src/blue/` | yes |

`just proto` regenerates TS + Python; CI fails if the committed output is stale.

**Versioning.** The `v1` in the package is the wire major version. Within v1, changes are
additive only (`buf breaking` against `main` once phase 2 lands). Handshakes also carry a
`protocol_version` integer so mismatched binaries fail fast with a readable error.

### 5.1 Daemon ⇄ Bridge (stdio, `blue.bridge.v1`)

One bridge process per session. **stdout carries only protocol frames**; stderr is logs
(daemon forwards them into `tracing`).

Framing: each frame is a protobuf varint length followed by that many bytes of message —
the standard "size-delimited" format (`prost::Message::encode_length_delimited` in Rust,
`sizeDelimitedEncode` / `sizeDelimitedDecodeStream` in `@bufbuild/protobuf/wire`).

- Daemon → bridge: `DaemonToBridge { oneof command { Start, Prompt, Close } }`
  - `Start { session_id, claude_path, resume_claude_session_id?, model?, cwd }` — `cwd` is where
    scouting starts for a new session, or the session's own directory when resuming
  - `Prompt { text }` — no turn id: the server sends a session's next prompt only after the
    current turn ends, so the daemon always knows which turn the bridge is running
  - `Close {}` (let the current turn finish, end the input stream, exit 0)
- Bridge → daemon: `BridgeToDaemon { oneof event { Ready, SdkMessage, Error, Exited } }`
  - `Ready { protocol_version, bridge_version, sdk_version, runtime_version }` — sent first
  - `SdkMessage { json }` — the `SDKMessage` exactly as the SDK yielded it, `JSON.stringify`'d.
    This is the one deliberate opaque field: the SDK's message shapes are Anthropic's and change
    with SDK releases, so the bridge doesn't reinterpret them; the daemon parses what it needs.
  - `Error { message }` — always non-fatal; the bridge keeps running
  - `Exited { error }` — last frame before exiting; `error` says why, empty on a clean shutdown.
    The daemon gets the exit status from the OS, and treats the bridge dying without `Exited`
    (crash or kill) as a failure, using the exit status and recent stderr as the reason

**Working directory: scout, then work.** A session's working directory decides which
`CLAUDE.md`, `.claude/` settings and git context Claude Code loads, and where it stores the
session for `resume` — and it can't change once the session starts (the SDK's `Query` has no way
to). So a new session runs in two steps:

1. **Scout.** On the first `Prompt`, the bridge runs a short throwaway query in `Start.cwd` (the
   daemon's starting directory; home by default) with Haiku, read-only tools (`Glob`, `Grep`,
   `Read`), no settings, no persistence, and one custom in-process tool,
   `set_project_directory(path)`. Its only job is to find the directory the request is about and
   call that tool. Its messages are logged, not forwarded. If it ends without choosing, the
   bridge sends a non-fatal `Error` and uses `Start.cwd`.
2. **Work.** The real session starts in the chosen directory with the same first prompt:
   `query({ prompt: inputQueue, options: { cwd, resume, model, pathToClaudeCodeExecutable,
   permissionMode: "bypassPermissions", allowDangerouslySkipPermissions: true,
   systemPrompt: { type: "preset", preset: "claude_code" } } })`, with all settings sources (the
   SDK default), so it behaves like Claude Code in that project. Claude can still work anywhere
   the OS user can. Follow-ups go into the same query; a task in another repo is a new session.

The daemon learns the chosen directory from the session's `init` message (`SessionStarted.cwd`)
and passes it back as `Start.cwd` when resuming, which skips scouting.

**No interrupts.** A turn runs until Claude finishes it. `Close` or stdin EOF lets the current
turn finish (scouting included), then ends the input so the query finishes cleanly. The only
way to stop a running turn is to kill the bridge, which fails the turn. Partial messages (`includePartialMessages`) stay off: nothing
consumes them, and voice only needs finished steps.

**Replay mode.** With `BLUE_BRIDGE_REPLAY=<recording>`, the bridge plays back one recorded turn
per `Prompt` instead of running Claude, so the daemon ⇄ bridge path is testable without a login.

Daemon-side process control: the bridge (and the `claude` it spawns) run in a Windows Job
Object / Unix process group so a hard kill takes down the whole tree.

### 5.2 Daemon ⇄ Server (`blue.daemon.v1.DaemonService`)

```proto
service DaemonService {
  rpc Attach(stream AttachRequest) returns (stream AttachResponse);
}
```

One long-lived bidirectional gRPC stream per daemon, opened outbound from the machine.
Auth: `authorization: Bearer <machine token>` metadata. HTTP/2 keepalive pings every 20 s;
the daemon reconnects with jittered backoff when the stream ends.

Daemon → server (`AttachRequest.oneof message`):

| Message | Fields | Notes |
|---|---|---|
| `Hello` | `protocol_version, versions{daemon, bridge, sdk, claude}, os, arch, sessions[{session_id, claude_session_id, state, last_seq}]` | must be first |
| `DaemonEvent` | `session_id, seq, turn_id, occurred_at, blue.events.v1.SessionEvent event` | `seq` per-session, monotonic, outbox-backed |
| `CommandResult` | `command_id, ok, error_code, error_message` | reply to a server `Command` |

Server → daemon (`AttachResponse.oneof message`):

| Message | Fields |
|---|---|
| `Welcome` | `machine_id, map<session_id, acked_seq>` — daemon trims/replays its outbox |
| `Command` | `command_id` + `oneof { StartSession, SendPrompt, CloseSession }` |
| `EventsAck` | `session_id, up_to_seq` — daemon deletes outbox rows ≤ seq |

- `StartSession { session_id, resume_claude_session_id?, model? }`
- `SendPrompt { session_id, turn_id, text }`
- `CloseSession { session_id }`

Delivery guarantees:

- Daemon → server events are **at-least-once**; server dedupes on `(session_id, seq)`.
- Server → daemon commands are idempotent by `command_id`; daemon keeps recently processed ids
  (persisted in its SQLite).
- On reconnect, `Hello.sessions` + `Welcome.acked_seqs` reconcile state: server marks sessions it
  believed running but the daemon doesn't know as failed ("daemon lost session"); daemon replays
  unacked outbox entries.

### 5.3 Client ⇄ Server (`blue.client.v1.ClientService`)

Used by voice, the CLI, and later the web UI (via gRPC-Web from `tonic-web`).
Auth: `authorization: Bearer <token>` — the voice service token or a user session token.

| RPC | Request → Response |
|---|---|
| `ListMachines` | — → machines with online status |
| `ListSessions` | `machine?, active_only` → sessions |
| `CreateSession` | `machine, instructions, title` → `session, turn_id` |
| `SendPrompt` | `session_id, instructions` → `turn_id` (queued if a turn is running) |
| `GetSession` | `session_id` → session + all its events (raw SDK messages excluded) |
| `Subscribe` (server stream) | `cursor?, session_ids[], include_raw` → stream of `Event` |

`Event { cursor, occurred_at, oneof kind { SessionEventEnvelope session, MachineStatus machine_status } }`.
`cursor` is the global `events.id`; a reconnecting subscriber passes its last cursor to resume.

`machine` accepts an id or name; the server resolves names case-insensitively. Errors
use gRPC status codes: `NOT_FOUND`, and `FAILED_PRECONDITION` for ambiguity, with an
`AmbiguousTarget { candidates[] }` message in the status details so the voice agent can ask.

### 5.4 Auth (`blue.auth.v1.AuthService`)

**GitHub is the only way to sign in.** There are no passwords. The server is registered as a
GitHub OAuth App and runs GitHub's
[device flow](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow)
on the client's behalf, so clients never handle GitHub tokens or OAuth secrets:

1. Client calls `LoginWithGitHub` (a server-streaming RPC).
2. Server requests a device code from GitHub and streams back `DeviceCode { user_code,
   verification_uri, expires_at }`. The CLI prints it (and opens the browser).
3. User enters the code at `github.com/login/device` and approves.
4. Server polls GitHub at the interval GitHub specifies, receives an access token, calls
   `GET /user`, then **discards the GitHub token**.
5. If the GitHub user id is in the allowlist, the server creates a Blue session and streams
   `LoginComplete { session_token, expires_at, user }`. Otherwise `PERMISSION_DENIED`.

The OAuth App requests no scopes; `/user` works without any. Device flow needs only the
client id (no secret). The allowlist is by GitHub numeric user id, which survives username
changes (`gh api user -q .id`).

A browser web UI (phase 7) would use the standard redirect flow instead, which needs the
client secret and one plain HTTPS callback endpoint on the server (GitHub redirects the browser
there, so it can't be gRPC). The session token it issues is the same.

| RPC | Auth | Purpose |
|---|---|---|
| `LoginWithGitHub` (server stream) | — | device flow → `DeviceCode`, then `LoginComplete` with a session token |
| `Logout` | session token | revoke the calling session |
| `GetCurrentUser` | session token | who am I |
| `CreateLiveKitToken` | session token | mint a room token with agent dispatch for `blue-voice` |
| `CreatePairingCode` | session token | one-time code, 10 min TTL |
| `ClaimPairingCode` | the code | daemon exchanges code + machine name → `machine_id`, machine token |
| `RevokeMachine` | session token | revoke a machine token and drop its stream |

Session tokens are opaque random values stored hashed in Postgres (not JWTs), so `Logout` and
revocation take effect immediately. Default lifetime 30 days.

### 5.5 Event model (`blue.events.v1.SessionEvent`)

The daemon translates raw `SDKMessage` JSON into `SessionEvent` (`crates/daemon/src/translate.rs`,
tested against recorded fixtures). `SessionEvent` is a `oneof`:

| Variant | From | Fields |
|---|---|---|
| `SessionStarted` | SDK `system` / `init` | `claude_session_id, model, cwd` |
| `TurnStarted` | daemon, on prompt sent | `turn_id` |
| `AssistantText` | SDK `assistant` text blocks | `text` (complete block, not partials) |
| `ToolStarted` | SDK `assistant` tool_use blocks | `tool_use_id, tool, summary` e.g. `Edit src/main.rs`, `Bash: cargo test` |
| `ToolFinished` | SDK `user` tool_result blocks | `tool_use_id, is_error` |
| `TurnCompleted` | SDK `result` (success) | `turn_id, result_text, duration_ms, num_turns, cost_usd` |
| `TurnFailed` | SDK `result` (error) / bridge crash | `turn_id, reason` |
| `SessionStateChanged` | daemon | `SessionState` enum: `STARTING, IDLE, RUNNING, CLOSED, FAILED` |
| `RawSdkMessage` | every SDK message (if `forward_raw`) | `json` |

`MachineStatus { machine_id, online }` is produced by the server, not the daemon, and lives in
`blue.client.v1.Event`. Unknown SDK message types are logged and forwarded raw, never fatal.

---

## 6. Components in detail

### 6.1 Server (`crates/server`, bin `blue-server`)

**Stack:** tokio, tonic (gRPC; `tonic-health` for probes, `tonic-web` later for the browser), sqlx (Postgres, compile-time-checked queries, embedded
migrations), `livekit-api` (token minting), `reqwest` (GitHub device flow + `/user`), `tracing`,
`clap`.

**Subcommands** (one image, three roles):

- `blue-server serve` — the server.
- `blue-server db bootstrap` — uses the admin DSN to create role + database (§7).
- `blue-server db migrate` — runs embedded migrations as the app role.

**gRPC services** (one port, h2c behind the ingress; TLS terminates at the ingress):

- `grpc.health.v1.Health` — liveness/readiness (readiness = DB reachable + migrations current);
  Kubernetes uses native gRPC probes.
- `blue.auth.v1.AuthService`, `blue.client.v1.ClientService`, `blue.daemon.v1.DaemonService`
  (§5). An interceptor maps the bearer token to a principal (user / voice / machine) and each
  RPC checks the principal kind it requires.

**Internals:**

- `routing::Registry` — in-memory map `machineId → daemon stream handle` (mpsc sender);
  online/offline derived from it and broadcast as `machine.status`.
- `routing::Fanout` — `tokio::sync::broadcast` of persisted events to client subscribers;
  slow subscribers get dropped and resume from their cursor.
- Write path: `DaemonEvent` → insert (`ON CONFLICT DO NOTHING` on `(session_id, seq)`) →
  update session/turn rows → broadcast → `events.ack` to daemon.
- Turn queue: if a `session.prompt` arrives while a turn is running, it is persisted as
  `queued` and dispatched on the next `turn.completed`/`failed`.
- Graceful shutdown on SIGTERM: health → NOT_SERVING, end daemon/client streams, flush, exit.

**Config (env):** `BLUE_DATABASE_URL`, `BLUE_PUBLIC_URL`,
`BLUE_VOICE_TOKEN` (compared in constant time; stored hashed at startup),
`BLUE_GITHUB_CLIENT_ID`, `BLUE_GITHUB_CLIENT_SECRET` (only needed for the phase 7 web flow),
`BLUE_GITHUB_ALLOWED_USER_IDS` (comma-separated), `BLUE_SESSION_TTL` (default `30d`),
`LIVEKIT_URL`, `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET`, `BLUE_AGENT_NAME` (default
`blue-voice`), `RUST_LOG`.

### 6.2 Database schema (Postgres)

```
users          (id, github_id BIGINT UNIQUE, github_login, name, avatar_url,
                created_at, last_login_at)
user_sessions  (id, user_id FK, token_hash UNIQUE, client_name, created_at,
                last_used_at, expires_at, revoked_at)
machines       (id, name UNIQUE, token_hash, os, arch, daemon_version, bridge_version,
                sdk_version, claude_version, created_at, last_seen_at, revoked_at)
pairing_codes  (code_hash PK, created_by, expires_at, claimed_at, machine_id)
sessions       (id, machine_id FK, title, state, claude_session_id,
                created_at, updated_at, last_activity_at)
turns          (id, session_id FK, index, instructions, state  -- queued|running|completed|failed
                , result_text, cost_usd, duration_ms, error, created_at, started_at, finished_at)
events         (id BIGSERIAL PK  -- global cursor
                , session_id FK, turn_id FK NULL, seq, kind, payload JSONB, is_raw BOOL,
                created_at, UNIQUE(session_id, seq))
audit_log      (id, at, actor, action, detail JSONB)
```

Raw events are retained for a configurable window (default 14 days) by a periodic cleanup
task; translated events are kept.

### 6.3 Daemon (`crates/daemon`, bin `blue-daemon`)

**Requirement: Claude Code is already installed and logged in.** Blue never bundles or installs
Claude. At startup the daemon resolves `claude` (from `claude_path` / `BLUE_CLAUDE_PATH`, else
PATH), runs `claude --version`, and refuses to start if either fails, with install instructions.
It reports the version in `Hello.versions.claude` and passes the resolved absolute path to every
bridge in `Start.claude_path`, so the Agent SDK never falls back to the copy of Claude Code it
ships as an npm dependency. (That copy is ~280 MB in `node_modules` on dev machines, but is not
compiled into the bridge.)

**Subcommands:**

- `blue-daemon pair --server https://server.example.com --code ABCD-1234 [--name desktop]`
- `blue-daemon start` — foreground.
- `blue-daemon service install|uninstall|status` — via the `service-manager` crate
  (Windows service / launchd / systemd user unit).
- `blue-daemon doctor` — checks: config valid, `claude` installed + version, bridge starts and
  reports a matching protocol version, server reachable and token accepted.

**Config:** `daemon.toml` in the OS config dir (`%APPDATA%\blue\` on Windows,
`~/.config/blue/` elsewhere):

```toml
server_url = "https://server.example.com"
machine_name = "desktop"
claude_path = "C:/Users/Jason/.local/bin/claude.exe"   # default: `claude` on PATH; required to exist
bridge_path = ""                                        # default: next to blue-daemon
start_directory = "~"                                   # where scouting starts (§5.1)
max_concurrent_sessions = 4
forward_raw = true
```

Machine token is stored in the OS keyring (`keyring` crate), falling back to a file with
restricted permissions.

**Internals:**

- `server_conn` — tonic client, open `Attach`, send `Hello`, reconnect with jittered exponential backoff, dispatch
  commands, drain outbox.
- `supervisor` — `session_id → Session` (phase 2).
- `session` (phase 1) — one bridge plus bookkeeping: the running turn (one at a time), per-session
  event `seq`, `DaemonEvent`s ready to send, the Claude session id and directory for resume, and
  a dead bridge turned into `TurnFailed` + `SessionState::FAILED` with the exit reason and recent
  stderr.
- `bridge_proc` (phase 1) — spawns the bridge in a Windows Job Object (kill-on-close) / Unix
  process group, size-delimited protobuf I/O, stderr forwarded to `tracing` (target `bridge`),
  kill-tree on drop.
- `translate` (phase 1) — raw `SDKMessage` → events (§5.5), snapshot-tested on recordings.
  Claude Code repeats `init` every turn (reported once); `total_cost_usd` is cumulative (each
  turn reports its difference); subagent messages are skipped; any unsuccessful `result` is
  `TurnFailed`.
- `recorder` (phase 1) — writes raw SDK messages to JSON lines, scrubbed for the public repo
  (home dir → `<HOME>`, `init` reduced to the fields Blue uses, `rate_limit_event` dropped).
- `outbox` — SQLite (`sqlx` sqlite): `(session_id, seq, payload)`; append before send,
  delete on `events.ack`. Also stores `sessions(session_id, claude_session_id)` so a
  restarted daemon can resume.
- Recovery: bridge exits unexpectedly mid-turn → emit `turn.failed`; next prompt to that
  session respawns a bridge with `resume: claudeSessionId`.
- Idle bridges are closed after a timeout (default 30 min) and transparently resumed on the
  next prompt.

### 6.4 Bridge (`bridge/`, exe `claude-bridge`)

- Bun + TypeScript; depends on `@anthropic-ai/claude-agent-sdk` (pinned exact version),
  `@bufbuild/protobuf`, and `zod` (input schema for the scouting tool).
- `main.ts` (protocol loop), `session.ts` (one streaming-input `query()` per process, input
  queue, forwards every message as `SdkMessage { json }`), `scout.ts` (§5.1), `replay.ts`,
  `io.ts`, `queue.ts`, `sdk.ts` (options shared by both queries).
- Always passes `pathToClaudeCodeExecutable` from `Start.claude_path` (the machine's installed
  `claude`); a `Start` without it is rejected, so the SDK's bundled copy is never used.
- Built with `bun build --compile` for `bun-windows-x64`, `bun-darwin-arm64`,
  `bun-darwin-x64`, `bun-linux-x64`, `bun-linux-arm64`; shipped next to `blue-daemon`.
- Replay mode (`BLUE_BRIDGE_REPLAY=<recording>`): plays back recorded turns instead of calling
  the SDK, for daemon tests and CI without Claude credentials.

### 6.5 Voice agent (`voice/`, Python)

- `livekit-agents` with LiveKit's recommended defaults at implementation time (the voice AI
  quickstart: STT/LLM/TTS via LiveKit Inference, Silero VAD, LiveKit turn detector, noise
  cancellation). No extra provider keys.
- Registered with explicit dispatch, `agent_name="blue-voice"`; the server's token endpoint puts
  the dispatch into the room token.
- Deployed with `lk agent create` / `lk agent deploy` from `voice/`. Secrets in LiveKit
  Cloud: `BLUE_SERVER_URL`, `BLUE_VOICE_TOKEN`.

**Per-job state:** one `ServerClient` (`grpc.aio` channel to `ClientService`; `Subscribe` stream
auto-reconnects and resumes from its last cursor), a "focused session" id, and a notification queue.

**Tools** (`@function_tool`):

| Tool | Does |
|---|---|
| `list_machines()` | machines and online status |
| `list_sessions(active_only=True)` | sessions with machine, title, state |
| `start_task(machine, title, instructions)` | `CreateSession`; the LLM names the session (a few words); focuses it |
| `send_follow_up(instructions, session=None)` | `SendPrompt` to focused or named session |
| `get_status(session=None)` | `GetSession`; LLM summarizes the latest activity |
| `focus_session(session)` | change focus by name/title/number |

**Behavior (system prompt essentials):**

- You are the voice front end to coding agents running on the user's machines. You don't
  write code yourself; you dispatch.
- Pass the user's request to Claude close to verbatim — clean up disfluencies, resolve "it"/
  "that" from context, don't reinterpret.
- Confirm the machine only when ambiguous (server returns candidates). If the user doesn't say
  which repo or directory to work in and context doesn't make it clear, ask before dispatching.
- Acknowledge dispatch in a few words. Never read code, diffs, or paths character by character.
- Stay silent during progress unless asked.

**Notifications (`notifier.py`):** subscribes to server events. On `TurnCompleted` /
`TurnFailed` / `MachineStatus{online=false}` with a running session, enqueue a notification.
When neither the user nor the agent is speaking, deliver it via
`session.generate_reply(instructions=…)` with the (truncated) `resultText` and the instruction
to summarize in ≤2 sentences. Multiple pending notifications are coalesced.

### 6.6 CLI (`crates/cli`, bin `blue`)

Text client over the same gRPC services (`ClientService` + `AuthService`):

- `blue login` (GitHub device flow; prints the code and opens github.com/login/device),
  `blue logout`, `blue whoami`; the session token is kept in the OS keyring
- `blue pair-code`, `blue machines [revoke <name>]`
- `blue run <machine> "<instructions>" [--title <title>]` (title defaults to the start of the
  instructions) — create session, stream events to terminal
- `blue send <session> "<instructions>"`, `blue sessions`,
  `blue tail [--session]`
- `blue token` — print a LiveKit room token (handy with Agents Playground)

---

## 7. Database bootstrap (external Postgres)

Assumption: a reachable Postgres server and an **admin DSN** whose role has `CREATEROLE` and
`CREATEDB`. Everything else the project creates itself.

`blue-server db bootstrap` (idempotent, safe to rerun):

1. Connect with admin DSN (`sslmode=require` default).
2. `CREATE ROLE blue_server LOGIN PASSWORD …` if missing; otherwise `ALTER ROLE … PASSWORD …`
   so the role password always matches the app Secret.
3. `CREATE DATABASE blue OWNER blue_server` if missing.
4. Connect to `blue`: `REVOKE ALL ON SCHEMA public FROM PUBLIC`,
   `ALTER SCHEMA public OWNER TO blue_server`.

`blue-server db migrate` then runs as `blue_server` — the admin role never touches app tables.

Fallback for providers where databases are created only through their own console/API:
`deploy/db/bootstrap.sql` (same steps, run by hand) and `database.bootstrap.enabled=false`.

---

## 8. Auth and security

Permissions are wide open on the machines, so **server auth is the security boundary**.

| Principal | Credential | Storage |
|---|---|---|
| Me (user) | GitHub sign-in (device flow) → Blue session token | allowlisted GitHub user id in config; session token hash in DB; token in OS keyring on the client |
| Voice agent | static service token | k8s Secret (server) + LiveKit Cloud secret (agent) |
| Each daemon | per-machine token from pairing | hash in DB; plaintext in OS keyring on machine |
| LiveKit room | room token minted by server after login | — |

- No passwords anywhere. GitHub tokens are used once to read `/user` and never stored.
- Pairing codes: single-use, 10-minute TTL, hashed at rest.
- Machine tokens revocable; revocation closes the live connection.
- Rate limiting on `AuthService.LoginWithGitHub` and `AuthService.ClaimPairingCode`.
- TLS everywhere via cert-manager on ingress; daemons refuse plaintext servers except
  `localhost`.
- No directory restrictions on the machines: Claude starts in the project scouting picks
  (§5.1) and can work anywhere the OS user can.
- Audit log of logins, pairings, revocations, session creates.

---

## 9. Kubernetes deployment (Helm chart `deploy/helm/blue-server`)

Target: self-run cluster (k3s assumed). Generic enough for others.

**Templates:**

- `deployment.yaml` — 1 replica, `strategy: Recreate`, non-root, read-only root FS,
  native gRPC liveness/readiness probes (`grpc.health.v1`), `terminationGracePeriodSeconds: 30`.
- `service.yaml` — ClusterIP, `appProtocol: kubernetes.io/h2c` and the Traefik
  `traefik.ingress.kubernetes.io/service.serversscheme: h2c` annotation so gRPC is proxied over HTTP/2.
- `ingress.yaml` — default `ingressClassName: traefik`, cert-manager annotation from values;
  optional ingress-nginx `backend-protocol: "GRPC"` + timeout annotations.
- `httproute.yaml` — optional Gateway API alternative.
- `secret-app.yaml` — generated voice token and DB app password; uses `lookup` to
  keep values stable across upgrades; created as a pre-install hook (weight −20) with
  `helm.sh/resource-policy: keep`. Skipped when `existingSecret` is set.
- `job-db-bootstrap.yaml` — `pre-install,pre-upgrade` hook, weight −10, runs
  `blue-server db bootstrap`, the only pod that mounts the admin DSN Secret.
- `job-db-migrate.yaml` — `pre-install,pre-upgrade` hook, weight 0, `blue-server db migrate`.
- `serviceaccount.yaml`, `NOTES.txt` (prints the server URL, how to get the voice token, how to
  pair a daemon), `tests/test-connection.yaml` (`helm test` runs a gRPC health check).
- `values.schema.json` — validates values.

**Values sketch:**

```yaml
image:
  repository: ghcr.io/<owner>/blue-server
  tag: ""                      # defaults to Chart appVersion
imagePullSecrets: []
replicaCount: 1
publicUrl: https://server.example.com
ingress:
  enabled: true
  className: traefik
  host: server.example.com
  tls: { enabled: true, clusterIssuer: letsencrypt-prod }
gatewayApi: { enabled: false }
database:
  host: ""
  port: 5432
  name: blue
  user: blue_server
  sslmode: require
  existingSecret: ""           # key: password
  bootstrap:
    enabled: true
    adminExistingSecret: ""    # key: dsn  (only mounted into the bootstrap Job)
livekit:
  url: wss://<project>.livekit.cloud
  existingSecret: ""           # keys: apiKey, apiSecret
  agentName: blue-voice
auth:
  github:
    clientId: ""               # GitHub OAuth App with device flow enabled
    allowedUserIds: []         # e.g. [12345678]  (gh api user -q .id)
    existingSecret: ""         # key: clientSecret (optional until the web UI)
  sessionTtl: 30d
  existingSecret: ""           # key: voiceToken
resources: { requests: { cpu: 50m, memory: 64Mi }, limits: { memory: 256Mi } }
```

**Image:** multi-stage with `cargo-chef`; runtime `gcr.io/distroless/cc-debian12:nonroot`;
multi-arch `linux/amd64` + `linux/arm64`. Published to GHCR. Chart published to GHCR as an
OCI artifact (`helm push`).

**Install:**

```sh
kubectl create secret generic blue-db-admin --from-literal=dsn='postgres://admin:…@host/postgres?sslmode=require'
kubectl create secret generic blue-livekit --from-literal=apiKey=… --from-literal=apiSecret=…
helm install blue oci://ghcr.io/<owner>/charts/blue-server \
  --set database.host=… --set database.bootstrap.adminExistingSecret=blue-db-admin \
  --set livekit.existingSecret=blue-livekit --set ingress.host=server.example.com
```

---

## 10. Local development

Tooling: Rust (stable, pinned via `rust-toolchain.toml`), Bun, uv, Docker, `just`, `lk` CLI,
k3d (for chart testing), Helm.

| Command | Does |
|---|---|
| `just db` | `docker compose up` Postgres 17 with an admin user |
| `just server` | `blue-server db bootstrap && db migrate && serve` against local Postgres |
| `just bridge` | `bun build --compile` the bridge into `target/bridge/` |
| `just daemon` | `blue-daemon start` against `http://localhost:8080` |
| `just voice` | `uv run python -m blue_voice dev` — local worker connects to LiveKit Cloud, talks to the local server directly (no tunnel needed) |
| `just proto` | `buf lint` + regenerate TS and Python protocol code (Rust regenerates on build) |
| `just session` | a Claude session in the terminal via `blue-daemon-test` (`--record` / `--replay`) |
| `just test` | cargo tests, bridge tests, voice tests |
| `just test-replay` | daemon ⇄ compiled bridge through a recorded session (no Claude login) |
| `just test-live` | real Claude sessions (needs a login; Haiku) |
| `just chart-test` | k3d cluster + in-cluster Postgres + `helm install` + `helm test` |

Frontend during development: LiveKit Agents Playground, connecting with a token from
`blue token`.

---

## 11. Testing strategy

- **Translation:** recorded SDK streams in `crates/daemon/tests/fixtures/*.jsonl` (captured from
  real Claude by the live tests, scrubbed), run through `translate.rs`, snapshot-tested with
  `insta`. `just record-fixtures` refreshes them; review with `cargo insta review`.
- **Live (opt-in, needs a Claude login, uses Haiku):** `just test-live` — real sessions through
  the compiled bridge: scouting, several turns, an edit, resume in a new bridge,
  and killing the bridge kills Claude and its shells (checked by PID *and* start time, since
  Windows reuses PIDs).
- **Protocol:** `buf lint`; framing round-trip tests in Rust and TS; Python import/round-trip
  test; generated-code freshness check in CI; cross-language handshake (daemon ⇄ compiled bridge).
- **Daemon:** `just test-replay` (in CI on Linux, macOS and Windows) — the daemon driving the
  compiled bridge in replay mode through a recorded multi-turn session. Phase 2
  adds outbox replay after a dropped server connection.
- **Server:** integration tests against real Postgres (`testcontainers`), with an in-process fake
  daemon and fake client over real gRPC streams: routing, dedupe, turn queueing, reconnect
  reconciliation, auth failures.
- **Bootstrap/migrations:** test against a fresh Postgres with a non-superuser admin that has
  only `CREATEROLE CREATEDB`.
- **Voice:** `livekit-agents` test harness with a mocked `ServerClient`: tool selection for
  representative utterances, ambiguity handling, notification delivery.
- **Chart:** `helm lint`, `ct lint`, k3d install in CI.
- **End-to-end smoke (manual / opt-in CI with credentials):** real `claude` on a scratch repo,
  driven by `blue run`.

Scaling note for later: multiple server replicas need cross-pod routing (daemon connected to pod
A, client on pod B). Postgres `LISTEN/NOTIFY` is the planned mechanism, so no Redis. Not in
scope now.

---

## 12. CI/CD (GitHub Actions)

- **ci.yml** (PRs + main): `cargo fmt --check`, `clippy -D warnings`, `cargo test` (with
  Postgres service), bridge `tsc --noEmit` + tests, voice `ruff` + `pyright` + `pytest`,
  `buf lint`, `just proto && git diff --exit-code`, `helm lint` + `ct lint`.
- **chart-e2e.yml**: k3d + Postgres + install chart + `helm test`.
- **release.yml** (tag `v*`):
  - server image → `ghcr.io/<owner>/blue-server:<version>` (amd64 + arm64)
  - chart → `oci://ghcr.io/<owner>/charts/blue-server:<version>`
  - daemon archives per target (`blue-daemon` + `claude-bridge` + `blue` + README) → GitHub
    Release
- Voice deploys are manual (`lk agent deploy`) at first; automate later if useful.

One version number for the whole repo.

---

## 13. Phases and exit criteria

**Phase 0 — Scaffold**
Cargo workspace, `proto/` with the v1 definitions, codegen for Rust/TS/Python, `bridge/`
(bun) answering the protobuf handshake, `voice/` (uv) skeleton, `justfile`, CI, GitHub repo.
*Exit:* `just test` and CI pass; `blue-daemon doctor` spawns the bun-compiled bridge and gets a
`Ready` frame over protobuf; `blue health` gets SERVING from `blue-server serve`.

**Phase 1 — Daemon ⇄ bridge on Windows (riskiest first)** ✅ *done*
Bridge implemented; Rust spawns it, runs a multi-turn session in a scratch repo, kills
mid-turn, resumes with `claudeSessionId`. Verify `bun build --compile` works with the
SDK and an explicit `pathToClaudeCodeExecutable`. Record fixtures. Implement `translate.rs`.
*Exit:* a Rust test binary drives three turns + resume against real `claude`;
translation snapshot tests pass on fixtures; process-tree kill verified on Windows.
*Result:* all met (`just test-live`, `just test-replay`, snapshot tests). Also added: scouting
for the working directory (§5.1), and replay mode in CI. Sessions are a library API
(`blue_daemon::session`, used by the tests and, from phase 2, the server connection); the
`blue-daemon` binary only exposes `doctor` so far. For trying sessions by hand there is a
separate, unshipped tool: `blue-daemon-test session [--cwd DIR] [--resume ID] [--model M]
[--record FILE | --replay FILE]` (`just session`), a Claude session in the terminal where each
line is a prompt and `/quit` ends it.

**Phase 2 — Server, protocol, daemon, CLI (local)**
Protocols in `crates/protocol`; server with Postgres, bootstrap, migrations, auth, pairing,
routing, event log, fan-out, turn queue; daemon server connection + outbox + reconciliation; CLI.
*Exit:* `blue run desktop "in the blue repo, …"` streams a full task through server → daemon → Claude and back;
killing the server or the daemon mid-task loses no events.

**Phase 3 — Voice agent (local worker)**
Agent with tools, server client, notifier, system prompt; LiveKit Cloud project; Agents
Playground with a server-minted token.
*Exit:* by voice alone: start a task, ask status, follow up, get a spoken completion
summary.

**Phase 4 — Helm chart**
Server Dockerfile, chart with hooks, k3d test, release workflow publishing image + chart.
*Exit:* `just chart-test` green locally and in CI against a fresh Postgres.

**Phase 5 — Real deployment**
Install chart in the self-run cluster against the hosted Postgres; DNS + TLS; `lk agent deploy`
the voice agent with secrets; pair this machine against the public server.
*Exit:* full voice loop from my phone with nothing running locally except `blue-daemon`.

**Phase 6 — Hardening**
Daemon as OS service with bundled bridge; second machine; offline handling and spoken
"machine went offline"; idle bridge reaping; raw-event retention; audit log; release archives
for all targets.
*Exit:* reboot a machine mid-session → it comes back, reconnects, and the session resumes on
the next prompt.

**Phase 7 — Web UI (later)**
GitHub sign-in (redirect flow), LiveKit client (JS SDK), live transcript, session/event view
from `raw` events.
Technology decided then.

---

## 14. Risks

| Risk | Mitigation |
|---|---|
| Agent SDK message shapes change | Pin SDK version; bridge forwards raw; lenient translation; fixtures + snapshot tests; deliberate upgrades |
| `bun build --compile` incompatibility with the SDK | Verify in phase 1; fallback: ship bridge as bundled JS + require Node, or Node SEA |
| Orphaned `claude` processes on Windows | Job Objects with kill-on-close; tested in phase 1 |
| Long-lived gRPC streams dropped by ingress/LB | HTTP/2 keepalive every 20 s; reconnect + outbox replay; documented ingress timeouts |
| Allow-all permissions, work anywhere, remote voice | Strong server auth; per-machine tokens; audit log |
| Voice talks over me / interrupts at bad times | Notifications only when both sides are quiet; coalescing; LiveKit turn detection |
| Hosted Postgres disallows `CREATE DATABASE` | Manual `bootstrap.sql` path with bootstrap disabled |
| `claude` not logged in on a machine | `blue-daemon doctor` checks; surfaced as `turn.failed` with a clear reason |

---

## 15. Open questions (none block phases 0–3)

1. ~~GitHub repo~~ — `u9g/blue` (public).
2. A GitHub OAuth App for sign-in (github.com/settings/developers → New OAuth App, enable
   device flow) and your GitHub user id for the allowlist. Needed in phase 2.
3. Server hostname and the cluster's cert-manager `ClusterIssuer` name.
4. Cluster node architecture (amd64, arm64, or both).
5. Hosted Postgres provider (to confirm `CREATEROLE`/`CREATEDB` are available).
6. LiveKit Cloud project to use.
