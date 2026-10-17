# Task runner for the whole monorepo. Recipes are single commands so they run under both
# sh (Linux/macOS/CI) and PowerShell (Windows).

set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

exe := if os_family() == "windows" { ".exe" } else { "" }

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

# Runner spawns the compiled bridge and completes the protobuf handshake
handshake: bridge
    cargo run -q -p blue-runner -- doctor --bridge target/bridge/claude-bridge{{exe}}

# Run the hub locally
hub:
    cargo run -p blue-hub -- serve --listen 127.0.0.1:50051

# gRPC health check against the hub
health:
    cargo run -q -p blue-cli -- health

# Run the voice agent as a local dev worker (needs voice/.env.local with LiveKit credentials)
[working-directory('voice')]
voice:
    uv run python -m blue_voice.agent dev
