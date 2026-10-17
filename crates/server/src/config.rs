//! Server configuration, from flags or `BLUE_*` / `LIVEKIT_*` environment variables.

use std::time::Duration;

/// Settings for `blue-server serve`.
#[derive(clap::Args, Clone, Debug)]
pub struct ServeConfig {
    /// Postgres DSN for the app role, e.g. postgres://blue_server:…@host/blue?sslmode=require.
    #[arg(long, env = "BLUE_DATABASE_URL", hide_env_values = true)]
    pub database_url: String,

    /// Static token the voice agent presents. Without it, voice can't connect.
    #[arg(long, env = "BLUE_VOICE_TOKEN", hide_env_values = true)]
    pub voice_token: Option<String>,

    #[command(flatten)]
    pub github: GitHubConfig,

    /// How long a sign-in lasts, e.g. `30d`.
    #[arg(long, env = "BLUE_SESSION_TTL", default_value = "30d", value_parser = humantime::parse_duration)]
    pub session_ttl: Duration,

    #[command(flatten)]
    pub livekit: LiveKitConfig,
}

#[derive(clap::Args, Clone, Debug)]
pub struct GitHubConfig {
    /// Client id of the GitHub OAuth App (device flow enabled). Without it, GitHub sign-in is
    /// unavailable.
    #[arg(long = "github-client-id", env = "BLUE_GITHUB_CLIENT_ID")]
    pub client_id: Option<String>,

    /// GitHub numeric user ids allowed to sign in (`gh api user -q .id`), comma-separated.
    #[arg(
        long = "github-allowed-user-ids",
        env = "BLUE_GITHUB_ALLOWED_USER_IDS",
        value_delimiter = ',',
        num_args = 0..
    )]
    pub allowed_user_ids: Vec<i64>,

    /// Base URL for github.com (device codes, access tokens). Overridable for tests.
    #[arg(
        long = "github-url",
        env = "BLUE_GITHUB_URL",
        default_value = "https://github.com",
        hide = true
    )]
    pub web_url: String,

    /// Base URL for the GitHub REST API. Overridable for tests.
    #[arg(
        long = "github-api-url",
        env = "BLUE_GITHUB_API_URL",
        default_value = "https://api.github.com",
        hide = true
    )]
    pub api_url: String,
}

#[derive(clap::Args, Clone, Debug)]
pub struct LiveKitConfig {
    /// LiveKit server URL handed to clients, e.g. wss://project.livekit.cloud.
    #[arg(long = "livekit-url", env = "LIVEKIT_URL")]
    pub url: Option<String>,

    #[arg(
        long = "livekit-api-key",
        env = "LIVEKIT_API_KEY",
        hide_env_values = true
    )]
    pub api_key: Option<String>,

    #[arg(
        long = "livekit-api-secret",
        env = "LIVEKIT_API_SECRET",
        hide_env_values = true
    )]
    pub api_secret: Option<String>,

    /// Voice agent name put into room tokens for explicit dispatch.
    #[arg(
        long = "agent-name",
        env = "BLUE_AGENT_NAME",
        default_value = "blue-voice"
    )]
    pub agent_name: String,
}
