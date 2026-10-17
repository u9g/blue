CREATE TABLE users (
    id            UUID PRIMARY KEY,
    github_id     BIGINT NOT NULL UNIQUE,
    github_login  TEXT NOT NULL,
    name          TEXT NOT NULL DEFAULT '',
    avatar_url    TEXT NOT NULL DEFAULT '',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE user_sessions (
    id           UUID PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash   BYTEA NOT NULL UNIQUE,
    client_name  TEXT NOT NULL DEFAULT '',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL,
    revoked_at   TIMESTAMPTZ
);

CREATE TABLE machines (
    id             UUID PRIMARY KEY,
    name           TEXT NOT NULL,
    token_hash     BYTEA NOT NULL UNIQUE,
    os             TEXT NOT NULL DEFAULT '',
    arch           TEXT NOT NULL DEFAULT '',
    daemon_version TEXT NOT NULL DEFAULT '',
    bridge_version TEXT NOT NULL DEFAULT '',
    sdk_version    TEXT NOT NULL DEFAULT '',
    claude_version TEXT NOT NULL DEFAULT '',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at   TIMESTAMPTZ,
    revoked_at     TIMESTAMPTZ
);

-- Names are matched case-insensitively; a revoked machine frees its name.
CREATE UNIQUE INDEX machines_name ON machines (lower(name)) WHERE revoked_at IS NULL;

CREATE TABLE pairing_codes (
    code_hash  BYTEA PRIMARY KEY,
    created_by UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    claimed_at TIMESTAMPTZ,
    machine_id UUID REFERENCES machines (id)
);

CREATE TABLE sessions (
    id                UUID PRIMARY KEY,
    machine_id        UUID NOT NULL REFERENCES machines (id),
    title             TEXT NOT NULL,
    -- blue.events.v1.SessionState
    state             SMALLINT NOT NULL,
    claude_session_id TEXT,
    -- The daemon has the session: it acknowledged StartSession or sent an event for it.
    delivered         BOOLEAN NOT NULL DEFAULT false,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_activity_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX sessions_machine ON sessions (machine_id, created_at);

CREATE TABLE turns (
    id            UUID PRIMARY KEY,
    session_id    UUID NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    index         INTEGER NOT NULL,
    instructions  TEXT NOT NULL,
    state         TEXT NOT NULL CHECK (state IN ('queued', 'running', 'completed', 'failed')),
    result_text   TEXT NOT NULL DEFAULT '',
    duration_ms   BIGINT,
    error         TEXT NOT NULL DEFAULT '',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- When the server sent the turn to the daemon; a queued turn with this set is in flight.
    dispatched_at TIMESTAMPTZ,
    started_at    TIMESTAMPTZ,
    finished_at   TIMESTAMPTZ,
    UNIQUE (session_id, index)
);

-- The event log. `id` is the global cursor clients resume from. Session events from daemons
-- carry the daemon's per-session `seq` (deduplicated here); events the server makes itself
-- (machine status, sessions a daemon lost) have no `seq`, and machine status has no session.
CREATE TABLE events (
    id          BIGSERIAL PRIMARY KEY,
    session_id  UUID REFERENCES sessions (id) ON DELETE CASCADE,
    turn_id     UUID,
    seq         BIGINT,
    kind        TEXT NOT NULL,
    -- Protobuf: blue.events.v1.SessionEvent, or blue.client.v1.MachineStatus.
    payload     BYTEA NOT NULL,
    is_raw      BOOLEAN NOT NULL DEFAULT false,
    occurred_at TIMESTAMPTZ NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (session_id, seq)
);

CREATE INDEX events_session ON events (session_id, id);

CREATE TABLE audit_log (
    id     BIGSERIAL PRIMARY KEY,
    at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    actor  TEXT NOT NULL,
    action TEXT NOT NULL,
    detail JSONB NOT NULL DEFAULT '{}'
);
