# syntax=docker/dockerfile:1
# Built by `lk agent create` / `lk agent deploy` from this directory. Adapted from LiveKit's
# agent-starter-python. The generated protobuf package (src/blue) is committed, so the build
# context does not need the rest of the monorepo.

ARG PYTHON_VERSION=3.13
FROM ghcr.io/astral-sh/uv:python${PYTHON_VERSION}-bookworm-slim AS base
ENV PYTHONUNBUFFERED=1 \
    UV_COMPILE_BYTECODE=1 \
    HF_HOME=/app/.cache/huggingface \
    TORCH_HOME=/app/.cache/torch

FROM base AS build
RUN apt-get update && apt-get install -y gcc g++ python3-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY pyproject.toml uv.lock ./
RUN uv sync --locked --no-dev --no-install-project
RUN uv run --no-sync --module livekit.agents download-files
COPY . .
RUN uv sync --locked --no-dev

FROM base
ARG UID=10001
RUN adduser --disabled-password --gecos "" --home /app --shell /sbin/nologin --uid "${UID}" appuser
COPY --from=build --chown=appuser:appuser /app /app
WORKDIR /app
USER appuser
CMD ["uv", "run", "--no-sync", "python", "-m", "blue_voice.agent", "start"]
