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
| `crates/cli` | `blue` text client | Rust |
| `bridge/` | `claude-bridge` Agent SDK adapter | TypeScript (Bun) |
| `voice/` | LiveKit voice agent | Python (uv) |

## Prerequisites

Rust (stable), [Bun](https://bun.sh), [uv](https://docs.astral.sh/uv/), and
[just](https://just.systems). No `protoc` install is needed: Rust uses a vendored `protoc`,
TypeScript uses `buf` from npm, Python uses `grpcio-tools`.

## Common tasks

```sh
just setup       # bun install + uv sync
just proto       # buf lint + regenerate TS/Python code from proto/
just lint        # all linters
just test        # all tests
just handshake   # build the bridge exe; daemon completes the protobuf handshake with it
just server         # run the server on 127.0.0.1:50051
just health      # gRPC health check against the server
just voice       # run the voice agent as a local dev worker (needs voice/.env.local)
```

Generated TypeScript (`bridge/src/gen`) and Python (`voice/src/blue`) code is committed; CI
fails if it is stale. Rust code is generated at build time.
