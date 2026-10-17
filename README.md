# Blue

Talk to Claude Code on your own machines by voice.

A LiveKit voice agent (Python, LiveKit Cloud) talks to the **server** (Rust, Kubernetes), which
routes work to a **daemon** (Rust) on each machine. The daemon drives the Claude Agent SDK
through a thin TypeScript **bridge**. Every link between these parts is Protocol Buffers;
the definitions in [`proto/`](proto) are the source of truth.

See [PLAN.md](PLAN.md) for the architecture and roadmap.

## Layout

| Path | What | Language |
|---|---|---|
| `proto/` | Protobuf definitions for every wire protocol | — |
| `crates/protocol` | Generated Rust types + stdio framing | Rust |
| `crates/server` | `blue-server` management server (gRPC) | Rust |
| `crates/daemon` | `blue-daemon` local client | Rust |
| `crates/daemon-test` | `blue-daemon-test` local testing tool (sessions in the terminal, record/replay) | Rust |
| `crates/cli` | `blue` text client | Rust |
| `crates/common` | Server connections and token storage shared by the daemon and CLI | Rust |
| `bridge/` | `claude-bridge` Agent SDK adapter | TypeScript (Bun) |
| `voice/` | LiveKit voice agent | Python (uv) |

## Prerequisites

Every machine that runs `blue-daemon` must already have
[Claude Code](https://code.claude.com/docs/en/setup) installed and logged in. Blue never bundles
Claude; `blue-daemon doctor` checks for it.

For development: Rust (stable), [Bun](https://bun.sh), [uv](https://docs.astral.sh/uv/), and
[just](https://just.systems). No `protoc` install is needed: Rust uses a vendored `protoc`,
TypeScript uses `buf` from npm, Python uses `grpcio-tools`.

## Common tasks

```sh
just setup       # bun install + uv sync
just proto       # buf lint + regenerate TS/Python code from proto/
just lint        # all linters
just test        # all tests
just handshake   # build the bridge exe; daemon completes the protobuf handshake with it
just session     # chat with Claude in this terminal via blue-daemon-test (--record / --replay)
just test-replay # daemon drives the compiled bridge through a recorded session (no login needed)
just test-live   # real Claude sessions: turns, resume, kill (needs a login; Haiku)
just test-e2e    # server + daemon processes, killing each mid-turn (no login needed)
just db          # Postgres 17 in Docker for local development
just server      # bootstrap + migrate the database, run the server on 127.0.0.1:50051
just dev-token   # a session token without GitHub sign-in (export it as BLUE_TOKEN)
just daemon      # run this machine's daemon (pair it first, see below)
just health      # gRPC health check against the server
just voice       # run the voice agent as a local dev worker (needs voice/.env.local)
```

Server tests need Postgres: they use `BLUE_TEST_DATABASE_URL` (an admin DSN) if set, otherwise
they download and start a throwaway Postgres with `postgresql_embedded`.

## Running it locally

```sh
just db                                   # or point BLUE_DATABASE_ADMIN_URL at any Postgres
just server                               # leave running
export BLUE_TOKEN=$(just dev-token)       # or `blue login` with a GitHub OAuth App configured
blue pair-code                            # prints a code
blue-daemon pair --server http://127.0.0.1:50051 --code <code> --name desktop
just daemon                               # leave running
blue run desktop "in the blue repo, add a health endpoint"
blue send "<session title or id>" "also add a test for it"
blue sessions; blue tail
```

Generated TypeScript (`bridge/src/gen`) and Python (`voice/src/blue`) code is committed; CI
fails if it is stale. Rust code is generated at build time.
