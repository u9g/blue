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
just server         # run the server on 127.0.0.1:50051
just health      # gRPC health check against the server
just voice       # run the voice agent as a local dev worker (needs voice/.env.local)
```

Generated TypeScript (`bridge/src/gen`) and Python (`voice/src/blue`) code is committed; CI
fails if it is stale. Rust code is generated at build time.
