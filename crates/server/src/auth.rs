//! Who is calling: bearer tokens to principals, plus the token and code helpers they rest on.
//!
//! Every token is an opaque random value; only its SHA-256 is stored, so revoking a row takes
//! effect on the next call.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use subtle::ConstantTimeEq;
use tonic::Status;
use tonic::metadata::MetadataMap;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct UserPrincipal {
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub github_login: String,
}

#[derive(Clone, Debug)]
pub struct MachinePrincipal {
    pub id: Uuid,
    pub name: String,
}

/// A caller of the client API: a signed-in user or the voice agent.
#[derive(Clone, Debug)]
pub enum Client {
    User(UserPrincipal),
    Voice,
}

impl Client {
    /// For the audit log.
    pub fn actor(&self) -> String {
        match self {
            Self::User(user) => format!("user:{}", user.github_login),
            Self::Voice => "voice".into(),
        }
    }
}

pub struct Authenticator {
    db: PgPool,
    voice_token_hash: Option<[u8; 32]>,
}

impl Authenticator {
    pub fn new(db: PgPool, voice_token: Option<&str>) -> Self {
        Self {
            db,
            voice_token_hash: voice_token.map(hash),
        }
    }

    pub async fn user(&self, metadata: &MetadataMap) -> Result<UserPrincipal, Status> {
        let token = bearer(metadata)?;
        self.lookup_user(token)
            .await?
            .ok_or_else(|| Status::unauthenticated("invalid or expired session token"))
    }

    pub async fn client(&self, metadata: &MetadataMap) -> Result<Client, Status> {
        let token = bearer(metadata)?;
        if let Some(expected) = &self.voice_token_hash
            && bool::from(hash(token).ct_eq(expected))
        {
            return Ok(Client::Voice);
        }
        match self.lookup_user(token).await? {
            Some(user) => Ok(Client::User(user)),
            None => Err(Status::unauthenticated("invalid or expired token")),
        }
    }

    pub async fn machine(&self, metadata: &MetadataMap) -> Result<MachinePrincipal, Status> {
        let token = bearer(metadata)?;
        let row: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT id, name FROM machines WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(hash(token).as_slice())
        .fetch_optional(&self.db)
        .await
        .map_err(internal)?;
        let (id, name) =
            row.ok_or_else(|| Status::unauthenticated("invalid or revoked machine token"))?;
        Ok(MachinePrincipal { id, name })
    }

    async fn lookup_user(&self, token: &str) -> Result<Option<UserPrincipal>, Status> {
        let row: Option<(Uuid, Uuid, String)> = sqlx::query_as(
            "UPDATE user_sessions s SET last_used_at = now()
             FROM users u
             WHERE s.token_hash = $1 AND s.user_id = u.id
               AND s.revoked_at IS NULL AND s.expires_at > now()
             RETURNING s.id, u.id, u.github_login",
        )
        .bind(hash(token).as_slice())
        .fetch_optional(&self.db)
        .await
        .map_err(internal)?;
        Ok(
            row.map(|(session_id, user_id, github_login)| UserPrincipal {
                user_id,
                session_id,
                github_login,
            }),
        )
    }
}

fn bearer(metadata: &MetadataMap) -> Result<&str, Status> {
    metadata
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| Status::unauthenticated("missing `authorization: Bearer <token>`"))
}

pub fn hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// A new random token, e.g. `blue_s_…` for user sessions, `blue_m_…` for machines.
pub fn new_token(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS random number generator");
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// Unambiguous characters (no 0/O, 1/I); 32 of them, so a random byte maps without bias.
const CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// A pairing code like `K7QM-2XPA`.
pub fn new_pairing_code() -> String {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("OS random number generator");
    let chars: String = bytes
        .iter()
        .map(|byte| CODE_ALPHABET[(byte % 32) as usize] as char)
        .collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

/// Codes are compared without case, spaces or dashes, so `k7qm 2xpa` matches `K7QM-2XPA`.
pub fn normalize_pairing_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Fixed-window limit on how often something may happen server-wide.
pub struct RateLimit {
    max: u32,
    window: Duration,
    state: Mutex<(Instant, u32)>,
}

impl RateLimit {
    pub fn new(max: u32, window: Duration) -> Self {
        Self {
            max,
            window,
            state: Mutex::new((Instant::now(), 0)),
        }
    }

    pub fn check(&self, what: &str) -> Result<(), Status> {
        let mut state = self.state.lock().unwrap();
        if state.0.elapsed() >= self.window {
            *state = (Instant::now(), 0);
        }
        if state.1 >= self.max {
            return Err(Status::resource_exhausted(format!(
                "too many {what} attempts; try again shortly"
            )));
        }
        state.1 += 1;
        Ok(())
    }
}

pub async fn audit(db: &PgPool, actor: &str, action: &str, detail: serde_json::Value) {
    let result = sqlx::query("INSERT INTO audit_log (actor, action, detail) VALUES ($1, $2, $3)")
        .bind(actor)
        .bind(action)
        .bind(detail)
        .execute(db)
        .await;
    if let Err(err) = result {
        tracing::error!(%actor, %action, "audit log write failed: {err}");
    }
}

pub fn internal(err: impl std::fmt::Display) -> Status {
    tracing::error!("{err}");
    Status::internal("internal error")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_codes() {
        let code = new_pairing_code();
        assert_eq!(code.len(), 9);
        assert_eq!(normalize_pairing_code(&code.to_lowercase()).len(), 8);
        assert_eq!(normalize_pairing_code(" k7qm 2xpa "), "K7QM2XPA");
    }

    #[test]
    fn rate_limit() {
        let limit = RateLimit::new(2, Duration::from_secs(60));
        assert!(limit.check("x").is_ok());
        assert!(limit.check("x").is_ok());
        assert!(limit.check("x").is_err());
    }
}
