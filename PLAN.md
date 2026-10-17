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
- Horizontal scaling of the server. One replica; see §10 for the path to more.

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
| daemon ⇄ server | gRPC over HTTP/2 + TLS (one bidirectional stream per daemon) | protobuf |
| voice ⇄ server | gRPC | protobuf |
| cli ⇄ server | gRPC (same services as voice) | protobuf |

The only JSON in the system is content we don't own: the Agent SDK's `SDKMessage` objects,
which ride inside protobuf as an opaque JSON string (§4.1).

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
                     │ gRPC (protobuf, service token)
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
               │ gRPC bidi stream (outbound, machine token)
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
3. Voice asks the server to create the session; the server stores the session and its first
   turn and tells the desktop daemon to start it; voice immediately says "On it."
4. The daemon spawns a bridge and hands it the prompt. The bridge finds the project (§4.1) and
   starts Claude there.
5. SDK messages stream bridge → daemon (raw). Daemon translates to events, writes them to its
   outbox, sends them to the server; server stores them and fans out to subscribers.
6. Voice stays quiet during progress. If I ask "what's it doing?", it calls `get_status`, which
   returns recent progress events, and summarizes.
7. When the turn completes, voice waits for a quiet moment and speaks a 1–2 sentence summary.
8. "Also add a test for it" → `send_follow_up` → same session, same bridge process, next turn.

---

## 4. Protocols

The `.proto` files under `proto/blue/<area>/v1/` are the source of truth for every message;
this plan doesn't repeat them. `buf lint` (STANDARD rules) runs in CI.

| Area | Link |
|---|---|
| `events` | session events shared by daemon, server and clients |
| `bridge` | daemon ⇄ bridge, over stdio |
| `daemon` | server ⇄ daemon |
| `client` | server ⇄ voice agent, CLI, later web |
| `auth` | GitHub sign-in, LiveKit tokens, daemon pairing |

Generated code:

| Language | Tool | Output | Committed? |
|---|---|---|---|
| Rust | `tonic-prost-build` in `crates/protocol/build.rs`, `protoc-bin-vendored` | `OUT_DIR` | no (built every time) |
| TypeScript | `buf generate` + `protoc-gen-es` (only `blue/bridge`) | `bridge/src/gen/` | yes |
| Python | `grpcio-tools` (`events`, `client`, `auth`) | `voice/src/blue/` | yes |

`just proto` regenerates TS + Python; CI fails if the committed output is stale.

**Versioning.** The `v1` in the package is the wire major version. Within v1, changes are
additive only (`buf breaking` against `main` once phase 2 lands). Handshakes also carry a
protocol version number so mismatched binaries fail fast with a readable error.

### 4.1 Daemon ⇄ Bridge (stdio)

One bridge process per session. **stdout carries only protocol frames**; stderr is logs
(daemon forwards them into `tracing`). Each frame is a protobuf varint length followed by that
many bytes of message — the standard "size-delimited" format.

The daemon starts a session, sends one prompt per turn, and finally asks the bridge to close.
The bridge reports that it's ready (with its versions), forwards every Agent SDK message
verbatim as JSON, reports non-fatal errors, and sends a final frame saying why it exited. SDK
messages stay opaque JSON on purpose: their shapes are Anthropic's and change between SDK
releases, so the bridge doesn't reinterpret them; the daemon parses what it needs. If the
bridge dies without its final frame (crash or kill), the daemon uses the OS exit status and
recent stderr as the reason.

**Working directory: scout, then work.** A session's working directory decides which
`CLAUDE.md`, `.claude/` settings and git context Claude Code loads, and where it stores the
session for `resume` — and it can't change once the session starts (the SDK has no way to). So
a new session runs in two steps:

1. **Scout.** On the first prompt, the bridge runs a short throwaway query in the daemon's
   starting directory (home by default) with Haiku, read-only tools (`Glob`, `Grep`, `Read`), no
   settings, no persistence, and one custom in-process tool, `set_project_directory(path)`. Its
   only job is to find the directory the request is about and call that tool. Its messages are
   logged, not forwarded. If it ends without choosing, the bridge reports a non-fatal error and
   uses the starting directory.
2. **Work.** The real session starts in the chosen directory with the same first prompt, using
   Claude Code's own system prompt and all settings sources (so the project's `CLAUDE.md`
   loads), `bypassPermissions`, and the machine's installed `claude`. Claude can still work
   anywhere the OS user can. Follow-ups go into the same query; a task in another repo is a new
   session.

The daemon learns the chosen directory when the session starts and passes it back when
resuming, which skips scouting.

**No interrupts.** A turn runs until Claude finishes it. Closing (or stdin EOF) lets the current
turn finish, scouting included, then ends the session cleanly. The only way to stop a running
turn is to kill the bridge, which fails the turn. Partial messages (`includePartialMessages`)
stay off: nothing consumes them, and voice only needs finished steps.

**Replay mode.** With `BLUE_BRIDGE_REPLAY=<recording>`, the bridge plays back one recorded turn
per prompt instead of running Claude, so the daemon ⇄ bridge path is testable without a login.

Daemon-side process control: the bridge (and the `claude` it spawns) run in a Windows Job
Object / Unix process group so a hard kill takes down the whole tree.

### 4.2 Daemon ⇄ Server

One long-lived bidirectional gRPC stream per daemon, opened outbound from the machine.
Auth: `authorization: Bearer <machine token>` metadata. HTTP/2 keepalive pings every 20 s;
the daemon reconnects with jittered backoff when the stream ends.

The daemon introduces itself first (versions, OS, the sessions it knows about), then streams
session events; the server sends commands (start a session, send a prompt, close a session)
and acknowledges stored events.

Delivery guarantees:

- Daemon → server events are **at-least-once**, numbered per session; the server dedupes on
  (session, sequence number).
- Server → daemon commands are idempotent by command id; the daemon keeps recently processed
  ids (persisted in its SQLite).
- On reconnect, the two sides compare what each has: the server marks sessions it believed
  running but the daemon doesn't know as failed ("daemon lost session"), and the daemon replays
  events the server hasn't acknowledged.

### 4.3 Client ⇄ Server

Used by voice, the CLI, and later the web UI (via gRPC-Web from `tonic-web`).
Auth: `authorization: Bearer <token>` — the voice service token or a user session token.

Clients can list machines and sessions, create a session (machine, instructions, title), send
a follow-up (queued if a turn is running), fetch a session with all its events, and subscribe
to a live event stream. Each event carries a global cursor; a reconnecting subscriber passes
its last cursor to resume without missing anything.

Machines can be named by id or name; the server resolves names case-insensitively and, when a
name is ambiguous, returns the candidates so the voice agent can ask.

### 4.4 Auth

**GitHub is the only way to sign in.** There are no passwords. The server is registered as a
GitHub OAuth App and runs GitHub's
[device flow](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow)
on the client's behalf, so clients never handle GitHub tokens or OAuth secrets:

1. The client starts a GitHub login (a server-streaming call).
2. The server requests a device code from GitHub and streams back the code and URL. The CLI
   prints it (and opens the browser).
3. The user enters the code at `github.com/login/device` and approves.
4. The server polls GitHub at the interval GitHub specifies, receives an access token, calls
   `GET /user`, then **discards the GitHub token**.
5. If the GitHub user id is in the allowlist, the server creates a Blue session and streams back
   a session token. Otherwise the login is refused.

The OAuth App requests no scopes; `/user` works without any. Device flow needs only the
client id (no secret). The allowlist is by GitHub numeric user id, which survives username
changes (`gh api user -q .id`).

A browser web UI (phase 7) would use the standard redirect flow instead, which needs the
client secret and one plain HTTPS callback endpoint on the server (GitHub redirects the browser
there, so it can't be gRPC). The session token it issues is the same.

With a session token, a user can also log out, mint a LiveKit room token that dispatches the
voice agent, create a one-time pairing code for a new daemon (10-minute TTL), and revoke a
machine. A daemon exchanges a pairing code for its machine token.

Session tokens are opaque random values stored hashed in Postgres (not JWTs), so logout and
revocation take effect immediately. Default lifetime 30 days.

### 4.5 Session events

The daemon translates raw Agent SDK messages into Blue session events, tested against recorded
fixtures. Events cover: the Claude session starting (with its model and directory), a turn
starting, complete assistant text, tool calls starting and finishing (each with a short
speakable summary like `Edit src/main.rs` or `Bash: Run tests`), a turn completing or failing,
and the session's state changing. The server adds machine online/offline events. Unknown SDK
message types are logged and forwarded raw, never fatal.

---

## 5. Components in detail

### 5.1 Server (`crates/server`, bin `blue-server`)

**Stack:** tokio, tonic (gRPC; `tonic-health` for probes, `tonic-web` later for the browser), sqlx (Postgres, runtime-checked queries covered by
the integration tests, embedded migrations), `livekit-api` (token minting), `reqwest` (GitHub device flow + `/user`), `tracing`,
`clap`.

**Subcommands** (one image, three roles):

- `blue-server serve` — the server.
- `blue-server db bootstrap` — uses the admin DSN to create role + database (§6).
- `blue-server db migrate` — runs embedded migrations as the app role.
- `blue-server token --github-id N` — issues a session token directly (local development and
  first-time setup without a GitHub OAuth App; needs the database credentials).

**gRPC services** (one port, h2c behind the ingress; TLS terminates at the ingress):

- `grpc.health.v1.Health` — liveness/readiness (readiness = DB reachable + migrations current);
  Kubernetes uses native gRPC probes.
- The auth, client and daemon services (§4). Each call requires a particular kind of
  caller: a user, the voice agent, or a machine.

**Config (env):** `BLUE_DATABASE_URL`, `BLUE_PUBLIC_URL`,
`BLUE_VOICE_TOKEN` (compared in constant time; stored hashed at startup),
`BLUE_GITHUB_CLIENT_ID`, `BLUE_GITHUB_CLIENT_SECRET` (only needed for the phase 7 web flow),
`BLUE_GITHUB_ALLOWED_USER_IDS` (comma-separated), `BLUE_SESSION_TTL` (default `30d`),
`LIVEKIT_URL`, `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET`, `BLUE_AGENT_NAME` (default
`blue-voice`), `RUST_LOG`.

### 5.2 Database schema (Postgres)

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
                , result_text, duration_ms, error, created_at, started_at, finished_at)
events         (id BIGSERIAL PK  -- global cursor
                , session_id FK NULL, turn_id NULL, seq NULL, kind, payload BYTEA, is_raw BOOL,
                occurred_at, created_at, UNIQUE(session_id, seq))
audit_log      (id, at, actor, action, detail JSONB)
```

Event payloads are the protobuf messages themselves (`SessionEvent`, or `MachineStatus` for
machine events, which have no session). Events the server makes itself (machine status, and
failing the turns of a session a daemon lost) have no `seq`. Every event insert goes through
one lock, so cursors become visible in order and a subscriber can't skip one. `sessions` also
has `delivered` (the daemon has the session) and `turns` has `dispatched_at` (sent to the
daemon, not yet started); reconnects resend what was dispatched but not started.

Raw events are retained for a configurable window (default 14 days) by a periodic cleanup
task; translated events are kept.

### 5.3 Daemon (`crates/daemon`, bin `blue-daemon`)

**Requirement: Claude Code is already installed and logged in.** Blue never bundles or installs
Claude. At startup the daemon resolves `claude` (from `claude_path` / `BLUE_CLAUDE_PATH`, else
PATH), runs `claude --version`, and refuses to start if either fails, with install instructions.
It reports the version to the server and passes the resolved absolute path to every bridge, so
the Agent SDK never falls back to the copy of Claude Code it ships as an npm dependency. (That copy is ~280 MB in `node_modules` on dev machines, but is not
compiled into the bridge.)

**Subcommands:**

- `blue-daemon pair --server https://server.example.com --code ABCD-1234 [--name desktop]`
- `blue-daemon start` — foreground.
- `blue-daemon service install|uninstall|status` — via the `service-manager` crate
  (Windows service / launchd / systemd user unit).
- `blue-daemon doctor` — checks: config valid, `claude` installed + version, bridge starts and
  reports a matching protocol version, server reachable and token accepted.

`--dir` / `BLUE_DAEMON_DIR` moves the config, token and state elsewhere (several daemons on one
box, tests); with it, the token is kept only in a file there.

**Config:** `daemon.toml` in the OS config dir (`%APPDATA%\blue\` on Windows,
`~/.config/blue/` elsewhere):

```toml
server_url = "https://server.example.com"
machine_name = "desktop"
claude_path = "C:/Users/Jason/.local/bin/claude.exe"   # default: `claude` on PATH; required to exist
bridge_path = ""                                        # default: next to blue-daemon
start_directory = "~"                                   # where scouting starts (§4.1)
forward_raw = true
```

Machine token is stored in the OS keyring (`keyring` crate) on Windows and macOS, falling back
to a file with restricted permissions; Linux always uses the file (the kernel keyring doesn't
survive reboots and headless machines have no Secret Service).

State lives in SQLite next to `daemon.toml`: the sessions the daemon owns (with their Claude
session id and directory, for resuming), an outbox of events the server hasn't acknowledged,
and processed command ids. Events are numbered and written to the outbox before they are sent.
On startup, a turn the old process was running is failed ("the daemon stopped...").

If a bridge exits unexpectedly mid-turn, that turn fails; the session's next prompt starts a
new bridge that resumes the same Claude conversation. Idle bridges are closed after a timeout
(default 30 min) and resumed the same way on the next prompt.

### 5.4 Bridge (`bridge/`, exe `claude-bridge`)

- Bun + TypeScript; depends on `@anthropic-ai/claude-agent-sdk` (pinned exact version),
  `@bufbuild/protobuf`, and `zod` (input schema for the scouting tool).
- Always runs the machine's installed `claude` path that the daemon passes in; it refuses to
  start a session without one, so the SDK's bundled copy is never used.
- Built with `bun build --compile` for `bun-windows-x64`, `bun-darwin-arm64`,
  `bun-darwin-x64`, `bun-linux-x64`, `bun-linux-arm64`; shipped next to `blue-daemon`.
- Replay mode (`BLUE_BRIDGE_REPLAY=<recording>`): plays back recorded turns instead of calling
  the SDK, for daemon tests and CI without Claude credentials.

### 5.5 Voice agent (`voice/`, Python)

- `livekit-agents` with LiveKit's recommended defaults at implementation time (the voice AI
  quickstart: STT/LLM/TTS via LiveKit Inference, Silero VAD, LiveKit turn detector, noise
  cancellation). No extra provider keys.
- Registered with explicit dispatch, `agent_name="blue-voice"`; the server's token endpoint puts
  the dispatch into the room token.
- Deployed with `lk agent create` / `lk agent deploy` from `voice/`. Secrets in LiveKit
  Cloud: `BLUE_SERVER_URL`, `BLUE_VOICE_TOKEN`.

**Tools** (`@function_tool`):

| Tool | Does |
|---|---|
| `list_machines()` | machines and online status |
| `list_sessions(active_only=True)` | sessions with machine, title, state |
| `start_task(machine, title, instructions)` | creates a session; the LLM names it (a few words); focuses it |
| `send_follow_up(instructions, session=None)` | sends a follow-up to the focused or named session |
| `get_status(session=None)` | fetches the session; the LLM summarizes the latest activity |
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

**Notifications:** the agent subscribes to server events. When a turn completes or fails, or a
machine with a running session goes offline, it queues a notification. When neither the user
nor the agent is speaking, it delivers it via `session.generate_reply(instructions=…)` with the
(truncated) result and the instruction to summarize in ≤2 sentences. Multiple pending
notifications are coalesced.

### 5.6 CLI (`crates/cli`, bin `blue`)

Text client over the same gRPC services as the voice agent, plus auth:

- `blue login` (GitHub device flow; prints the code and opens github.com/login/device),
  `blue logout`, `blue whoami`; the session token is kept in the OS keyring
- `blue pair-code`, `blue machines [revoke <name>]`
- `blue run <machine> "<instructions>" [--title <title>]` (title defaults to the start of the
  instructions) — create session, stream events to terminal
- `blue send <session> "<instructions>"`, `blue sessions`,
  `blue tail [--session]`
- `blue token` — print a LiveKit room token (handy with Agents Playground)

---

## 6. Database bootstrap (external Postgres)

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

## 7. Auth and security

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
- Rate limiting on GitHub logins and pairing-code claims.
- TLS everywhere via cert-manager on ingress; daemons refuse plaintext servers except
  `localhost`.
- No directory restrictions on the machines: Claude starts in the project scouting picks
  (§4.1) and can work anywhere the OS user can.
- Audit log of logins, pairings, revocations, session creates.

---

## 8. Kubernetes deployment (Helm chart `deploy/helm/blue-server`)

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

## 9. Local development

Tooling: Rust (stable, pinned via `rust-toolchain.toml`), Bun, uv, Docker, `just`, `lk` CLI,
k3d (for chart testing), Helm.

| Command | Does |
|---|---|
| `just db` | `docker compose up` Postgres 17 with an admin user |
| `just server` | `blue-server db bootstrap && db migrate && serve` against local Postgres |
| `just dev-token` | a session token without GitHub sign-in, for `BLUE_TOKEN` |
| `just bridge` | `bun build --compile` the bridge into `target/bridge/` |
| `just daemon` | `blue-daemon start` against `http://localhost:8080` |
| `just voice` | `uv run python -m blue_voice dev` — local worker connects to LiveKit Cloud, talks to the local server directly (no tunnel needed) |
| `just proto` | `buf lint` + regenerate TS and Python protocol code (Rust regenerates on build) |
| `just session` | a Claude session in the terminal via `blue-daemon-test` (`--record` / `--replay`) |
| `just test` | cargo tests, bridge tests, voice tests |
| `just test-replay` | daemon ⇄ compiled bridge through a recorded session (no Claude login) |
| `just test-e2e` | server + daemon processes + compiled bridge in replay mode, killing each mid-turn |
| `just test-live` | real Claude sessions (needs a login; Haiku) |
| `just chart-test` | k3d cluster + in-cluster Postgres + `helm install` + `helm test` |

Frontend during development: LiveKit Agents Playground, connecting with a token from
`blue token`.

---

## 10. Testing strategy

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
  compiled bridge in replay mode through a recorded multi-turn session.
- **Server:** integration tests against real Postgres, with an in-process fake daemon and fake
  client over real gRPC streams: routing, dedupe, turn queueing, reconnect reconciliation, auth
  failures, GitHub sign-in against a fake GitHub. Postgres is `BLUE_TEST_DATABASE_URL` (a
  service container in CI) or a throwaway server from `postgresql_embedded` (no Docker needed).
- **End to end:** `just test-e2e` (in CI on Linux, macOS and Windows): real `blue-server` and
  `blue-daemon` processes, the compiled bridge replaying slowly; kills the server mid-turn, then
  the daemon, and checks every event arrived exactly once.
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

## 11. CI/CD (GitHub Actions)

- **ci.yml** (PRs + main): `cargo fmt --check`, `clippy -D warnings`, `cargo test` (with
  Postgres service), bridge `tsc --noEmit` + tests, voice `ruff` + `pyright` + `pytest`,
  `buf lint`, `buf breaking` against `main`, `just proto && git diff --exit-code`, `helm lint` +
  `ct lint`.
- **chart-e2e.yml**: k3d + Postgres + install chart + `helm test`.
- **release.yml** (tag `v*`):
  - server image → `ghcr.io/<owner>/blue-server:<version>` (amd64 + arm64)
  - chart → `oci://ghcr.io/<owner>/charts/blue-server:<version>`
  - daemon archives per target (`blue-daemon` + `claude-bridge` + `blue` + README) → GitHub
    Release
- Voice deploys are manual (`lk agent deploy`) at first; automate later if useful.

One version number for the whole repo.

---

## 12. Phases and exit criteria

**Phase 0 — Scaffold**
Cargo workspace, `proto/` with the v1 definitions, codegen for Rust/TS/Python, `bridge/`
(bun) answering the protobuf handshake, `voice/` (uv) skeleton, `justfile`, CI, GitHub repo.
*Exit:* `just test` and CI pass; `blue-daemon doctor` spawns the bun-compiled bridge and gets a
ready frame over protobuf; `blue health` gets SERVING from `blue-server serve`.

**Phase 1 — Daemon ⇄ bridge on Windows (riskiest first)** ✅ *done*
Bridge implemented; Rust spawns it, runs a multi-turn session in a scratch repo, kills
mid-turn, resumes with the Claude session id. Verify `bun build --compile` works with the
SDK and an explicit `pathToClaudeCodeExecutable`. Record fixtures. Implement `translate.rs`.
*Exit:* a Rust test binary drives three turns + resume against real `claude`;
translation snapshot tests pass on fixtures; process-tree kill verified on Windows.
*Result:* all met (`just test-live`, `just test-replay`, snapshot tests). Also added: scouting
for the working directory (§4.1), and replay mode in CI. Sessions are a library API
(`blue_daemon::session`, used by the tests and, from phase 2, the server connection); the
`blue-daemon` binary only exposes `doctor` so far. For trying sessions by hand there is a
separate, unshipped tool: `blue-daemon-test session [--cwd DIR] [--resume ID] [--model M]
[--record FILE | --replay FILE]` (`just session`), a Claude session in the terminal where each
line is a prompt and `/quit` ends it.

**Phase 2 — Server, protocol, daemon, CLI (local)** ✅ *done*
Protocols in `crates/protocol`; server with Postgres, bootstrap, migrations, auth, pairing,
routing, event log, fan-out, turn queue; daemon server connection + outbox + reconciliation; CLI.
*Exit:* `blue run desktop "in the blue repo, …"` streams a full task through server → daemon → Claude and back;
killing the server or the daemon mid-task loses no events.
*Result:* met. `just test-e2e` kills the server mid-turn (the turn finishes and every event
arrives once) and the daemon mid-turn (that turn fails, the next resumes the conversation);
`blue run`/`blue send` ran real Claude turns through a local server and daemon. Also: LiveKit
room tokens (`blue token`), `blue-server token` for working without a GitHub OAuth App, a
`GetMachine` RPC for `doctor`'s token check, and `buf breaking` in CI. Not yet: a limit on
concurrent sessions per daemon, raw-event retention, idle bridge reaping (phase 6).

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

## 13. Risks

| Risk | Mitigation |
|---|---|
| Agent SDK message shapes change | Pin SDK version; bridge forwards raw; lenient translation; fixtures + snapshot tests; deliberate upgrades |
| `bun build --compile` incompatibility with the SDK | Verify in phase 1; fallback: ship bridge as bundled JS + require Node, or Node SEA |
| Orphaned `claude` processes on Windows | Job Objects with kill-on-close; tested in phase 1 |
| Long-lived gRPC streams dropped by ingress/LB | HTTP/2 keepalive every 20 s; reconnect + outbox replay; documented ingress timeouts |
| Allow-all permissions, work anywhere, remote voice | Strong server auth; per-machine tokens; audit log |
| Voice talks over me / interrupts at bad times | Notifications only when both sides are quiet; coalescing; LiveKit turn detection |
| Hosted Postgres disallows `CREATE DATABASE` | Manual `bootstrap.sql` path with bootstrap disabled |
| `claude` not logged in on a machine | `blue-daemon doctor` checks; surfaced as a failed turn with a clear reason |

---

## 14. Open questions (none block phases 0–3)

1. ~~GitHub repo~~ — `u9g/blue` (public).
2. A GitHub OAuth App for sign-in (github.com/settings/developers → New OAuth App, enable
   device flow) and your GitHub user id for the allowlist. Needed in phase 2.
3. Server hostname and the cluster's cert-manager `ClusterIssuer` name.
4. Cluster node architecture (amd64, arm64, or both).
5. Hosted Postgres provider (to confirm `CREATEROLE`/`CREATEDB` are available).
6. LiveKit Cloud project to use.
