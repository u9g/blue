# Task runner for the whole monorepo. Recipes are single commands so they run under both
# sh (Linux/macOS/CI) and PowerShell (Windows).

set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

exe := if os_family() == "windows" { ".exe" } else { "" }

# Local development database (`just db`). Override either for another Postgres.
export BLUE_DATABASE_ADMIN_URL := env_var_or_default("BLUE_DATABASE_ADMIN_URL", "postgres://postgres:postgres@localhost:5432/postgres?sslmode=disable")
export BLUE_DATABASE_URL := env_var_or_default("BLUE_DATABASE_URL", "postgres://blue_server:blue-dev@localhost:5432/blue?sslmode=disable")

default:
    @just --list

# Install JS and Python dependencies
setup:
    bun install
    uv sync --project voice

# Lint protos and regenerate TypeScript + Python code (Rust regenerates on every build)
proto: proto-lint proto-ts proto-py

proto-lint:
    bun run proto:lint

proto-ts:
    bun run proto:ts

proto-py:
    uv run --project voice python voice/scripts/gen_proto.py

# Build all Rust binaries and the bridge
build: bridge
    cargo build --workspace

# Compile the bridge into a standalone executable at target/bridge/
[working-directory('bridge')]
bridge:
    bun run build

test: test-rust test-bridge test-voice

test-rust:
    cargo test --workspace

[working-directory('bridge')]
test-bridge:
    bun test

[working-directory('voice')]
test-voice:
    uv run pytest -q

lint: proto-lint lint-rust lint-bridge lint-voice

lint-rust:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

[working-directory('bridge')]
lint-bridge:
    bun run typecheck

[working-directory('voice')]
lint-voice:
    uv run ruff check
    uv run ruff format --check

# Daemon spawns the compiled bridge and completes the protobuf handshake
handshake: bridge
    cargo run -q -p blue-daemon -- doctor --bridge target/bridge/claude-bridge{{exe}}

# Chat with Claude in this terminal via blue-daemon-test (add --model, --record <file>, --replay <file>)
session *args: bridge
    cargo run -q -p blue-daemon-test -- session --bridge target/bridge/claude-bridge{{exe}} {{args}}

# Daemon driving the compiled bridge in replay mode (no Claude needed)
test-replay: bridge
    cargo test -q -p blue-daemon --test replay -- --ignored

# Real Claude sessions: needs Claude Code installed and logged in; uses Haiku, costs a little
test-live: bridge
    cargo test -q -p blue-daemon --test live -- --ignored --test-threads=1

# test-live, then copy its recordings into the fixtures (review snapshots with `cargo insta review`)
record-fixtures: test-live
    cp target/fixtures/*.jsonl crates/daemon/tests/fixtures/

# Server + daemon processes + replaying bridge; kills each mid-turn (no Claude login; own Postgres)
test-e2e: bridge
    cargo build -q -p blue-daemon
    cargo test -q -p blue-server --test e2e -- --ignored

# Postgres 17 for local development, in Docker
db:
    docker compose up -d --wait postgres

# Create the database, migrate it, and run the server locally
server:
    cargo run -q -p blue-server -- db bootstrap
    cargo run -q -p blue-server -- db migrate
    cargo run -p blue-server -- serve --listen 127.0.0.1:50051

# Print a session token for local development, without GitHub sign-in (use as BLUE_TOKEN)
dev-token github_id="1":
    cargo run -q -p blue-server -- token --github-id {{github_id}}

# Run this machine's daemon against the server it is paired with (`blue-daemon pair` first)
daemon: bridge
    cargo run -p blue-daemon -- start --bridge target/bridge/claude-bridge{{exe}}

# gRPC health check against the server
health:
    cargo run -q -p blue-cli -- health

# Run the voice agent as a local dev worker (needs voice/.env.local with LiveKit credentials)
[working-directory('voice')]
voice:
    uv run python -m blue_voice.agent dev
