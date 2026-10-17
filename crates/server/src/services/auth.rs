//! AuthService: GitHub sign-in, LiveKit room tokens, and pairing/revoking daemons.

use std::pin::Pin;
use std::sync::Arc;

use blue_protocol::auth::v1::auth_service_server::AuthService;
use blue_protocol::auth::v1::{
    ClaimPairingCodeRequest, ClaimPairingCodeResponse, CreateLiveKitTokenRequest,
    CreateLiveKitTokenResponse, CreatePairingCodeRequest, CreatePairingCodeResponse, DeviceCode,
    GetCurrentUserRequest, GetCurrentUserResponse, LoginComplete, LoginWithGitHubRequest,
    LoginWithGitHubResponse, LogoutRequest, LogoutResponse, RevokeMachineRequest,
    RevokeMachineResponse, User, login_with_git_hub_response,
};
use chrono::{Duration, Utc};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::AppState;
use crate::auth::{audit, hash, internal, new_pairing_code, new_token, normalize_pairing_code};
use crate::github::{GitHubUser, LoginError};
use crate::livekit::RoomToken;
use crate::services::client::resolve_machine;
use crate::time::timestamp;

const PAIRING_CODE_TTL: Duration = Duration::minutes(10);

pub struct Auth(pub Arc<AppState>);

type LoginStream = Pin<Box<dyn Stream<Item = Result<LoginWithGitHubResponse, Status>> + Send>>;

#[tonic::async_trait]
impl AuthService for Auth {
    type LoginWithGitHubStream = LoginStream;

    async fn login_with_git_hub(
        &self,
        request: Request<LoginWithGitHubRequest>,
    ) -> Result<Response<LoginStream>, Status> {
        let state = self.0.clone();
        let github = state.github.as_ref().ok_or_else(|| {
            Status::unavailable("GitHub sign-in is not configured on this server")
        })?;
        state.login_limit.check("sign-in")?;
        let client_name = request.into_inner().client_name;
        let code = github.device_code().await.map_err(|err| {
            tracing::error!("GitHub device code: {err:#}");
            Status::unavailable("could not reach GitHub")
        })?;

        let (tx, rx) = mpsc::channel(2);
        let first = LoginWithGitHubResponse {
            step: Some(login_with_git_hub_response::Step::DeviceCode(DeviceCode {
                user_code: code.user_code.clone(),
                verification_uri: code.verification_uri.clone(),
                expires_at: Some(timestamp(
                    Utc::now() + Duration::seconds(code.expires_in as i64),
                )),
            })),
        };
        let _ = tx.send(Ok(first)).await;
        tokio::spawn(async move {
            let github = state.github.as_ref().expect("checked above");
            let result = tokio::select! {
                result = github.wait_for_user(&code) => result,
                // Stop polling GitHub if the client gives up.
                () = tx.closed() => return,
            };
            let response = match result {
                Ok(user) => complete_login(&state, user, &client_name).await,
                Err(LoginError::Expired) => Err(Status::deadline_exceeded("the code expired")),
                Err(LoginError::Denied) => Err(Status::aborted("sign-in was declined on GitHub")),
                Err(LoginError::Other(err)) => {
                    tracing::error!("GitHub device flow: {err:#}");
                    Err(Status::unavailable("GitHub sign-in failed"))
                }
            };
            let _ = tx.send(response).await;
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn logout(
        &self,
        request: Request<LogoutRequest>,
    ) -> Result<Response<LogoutResponse>, Status> {
        let state = &self.0;
        let user = state.auth.user(request.metadata()).await?;
        sqlx::query("UPDATE user_sessions SET revoked_at = now() WHERE id = $1")
            .bind(user.session_id)
            .execute(&state.db)
            .await
            .map_err(internal)?;
        audit(
            &state.db,
            &format!("user:{}", user.github_login),
            "logout",
            json!({}),
        )
        .await;
        Ok(Response::new(LogoutResponse {}))
    }

    async fn get_current_user(
        &self,
        request: Request<GetCurrentUserRequest>,
    ) -> Result<Response<GetCurrentUserResponse>, Status> {
        let state = &self.0;
        let principal = state.auth.user(request.metadata()).await?;
        let (github_id, github_login, name, avatar_url): (i64, String, String, String) =
            sqlx::query_as(
                "SELECT github_id, github_login, name, avatar_url FROM users WHERE id = $1",
            )
            .bind(principal.user_id)
            .fetch_one(&state.db)
            .await
            .map_err(internal)?;
        Ok(Response::new(GetCurrentUserResponse {
            user: Some(User {
                github_id,
                github_login,
                name,
                avatar_url,
            }),
        }))
    }

    async fn create_live_kit_token(
        &self,
        request: Request<CreateLiveKitTokenRequest>,
    ) -> Result<Response<CreateLiveKitTokenResponse>, Status> {
        let state = &self.0;
        let user = state.auth.user(request.metadata()).await?;
        let livekit = &state.config.livekit;
        let (Some(url), Some(api_key), Some(api_secret)) =
            (&livekit.url, &livekit.api_key, &livekit.api_secret)
        else {
            return Err(Status::unavailable(
                "LiveKit is not configured on this server",
            ));
        };
        let room_name = request
            .into_inner()
            .room_name
            .filter(|room| !room.is_empty())
            .unwrap_or_else(|| format!("blue-{}", Uuid::new_v4().simple()));
        let token = RoomToken {
            api_key,
            api_secret,
            identity: &format!("user-{}", user.github_login),
            name: &user.github_login,
            room: &room_name,
            agent_name: &livekit.agent_name,
        }
        .sign(Utc::now().timestamp());
        Ok(Response::new(CreateLiveKitTokenResponse {
            token,
            livekit_url: url.clone(),
            room_name,
        }))
    }

    async fn create_pairing_code(
        &self,
        request: Request<CreatePairingCodeRequest>,
    ) -> Result<Response<CreatePairingCodeResponse>, Status> {
        let state = &self.0;
        let user = state.auth.user(request.metadata()).await?;
        let code = new_pairing_code();
        let expires_at = Utc::now() + PAIRING_CODE_TTL;
        sqlx::query(
            "INSERT INTO pairing_codes (code_hash, created_by, expires_at) VALUES ($1, $2, $3)",
        )
        .bind(hash(&normalize_pairing_code(&code)).as_slice())
        .bind(user.user_id)
        .bind(expires_at)
        .execute(&state.db)
        .await
        .map_err(internal)?;
        audit(
            &state.db,
            &format!("user:{}", user.github_login),
            "pairing_code.create",
            json!({}),
        )
        .await;
        Ok(Response::new(CreatePairingCodeResponse {
            code,
            expires_at: Some(timestamp(expires_at)),
        }))
    }

    async fn claim_pairing_code(
        &self,
        request: Request<ClaimPairingCodeRequest>,
    ) -> Result<Response<ClaimPairingCodeResponse>, Status> {
        let state = &self.0;
        state.claim_limit.check("pairing")?;
        let request = request.into_inner();
        let name = request.machine_name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err(Status::invalid_argument(
                "machine_name must be 1 to 64 characters",
            ));
        }
        let code_hash = hash(&normalize_pairing_code(&request.code));
        let machine_id = Uuid::now_v7();
        let token = new_token("blue_m_");

        let mut tx = state.db.begin().await.map_err(internal)?;
        let valid: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM pairing_codes
             WHERE code_hash = $1 AND claimed_at IS NULL AND expires_at > now()
             FOR UPDATE",
        )
        .bind(code_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?;
        if valid.is_none() {
            return Err(Status::permission_denied(
                "invalid, expired or already used pairing code",
            ));
        }
        let inserted =
            sqlx::query("INSERT INTO machines (id, name, token_hash) VALUES ($1, $2, $3)")
                .bind(machine_id)
                .bind(name)
                .bind(hash(&token).as_slice())
                .execute(&mut *tx)
                .await;
        match inserted {
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
                return Err(Status::already_exists(format!(
                    "a machine named {name:?} already exists; pick another name or revoke it first"
                )));
            }
            other => other.map_err(internal)?,
        };
        sqlx::query(
            "UPDATE pairing_codes SET claimed_at = now(), machine_id = $2 WHERE code_hash = $1",
        )
        .bind(code_hash.as_slice())
        .bind(machine_id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        audit(
            &state.db,
            &format!("machine:{name}"),
            "machine.pair",
            json!({ "machine_id": machine_id }),
        )
        .await;
        Ok(Response::new(ClaimPairingCodeResponse {
            machine_id: machine_id.to_string(),
            machine_token: token,
        }))
    }

    async fn revoke_machine(
        &self,
        request: Request<RevokeMachineRequest>,
    ) -> Result<Response<RevokeMachineResponse>, Status> {
        let state = &self.0;
        let user = state.auth.user(request.metadata()).await?;
        let (machine_id, name) = resolve_machine(state, &request.into_inner().machine).await?;
        sqlx::query("UPDATE machines SET revoked_at = now() WHERE id = $1")
            .bind(machine_id)
            .execute(&state.db)
            .await
            .map_err(internal)?;
        state.hub.kick(machine_id);
        audit(
            &state.db,
            &format!("user:{}", user.github_login),
            "machine.revoke",
            json!({ "machine_id": machine_id, "name": name }),
        )
        .await;
        Ok(Response::new(RevokeMachineResponse {}))
    }
}

async fn complete_login(
    state: &AppState,
    github_user: GitHubUser,
    client_name: &str,
) -> Result<LoginWithGitHubResponse, Status> {
    if !state
        .config
        .github
        .allowed_user_ids
        .contains(&github_user.id)
    {
        tracing::warn!(login = %github_user.login, id = github_user.id, "GitHub account not allowed");
        audit(
            &state.db,
            &format!("github:{}", github_user.login),
            "login.denied",
            json!({ "github_id": github_user.id }),
        )
        .await;
        return Err(Status::permission_denied(format!(
            "GitHub account {} is not allowed on this server",
            github_user.login
        )));
    }
    let (token, expires_at, user) = create_session(
        &state.db,
        state.config.session_ttl,
        &github_user,
        client_name,
    )
    .await?;
    audit(
        &state.db,
        &format!("user:{}", github_user.login),
        "login",
        json!({ "client_name": client_name }),
    )
    .await;
    Ok(LoginWithGitHubResponse {
        step: Some(login_with_git_hub_response::Step::Complete(LoginComplete {
            session_token: token,
            expires_at: Some(timestamp(expires_at)),
            user: Some(user),
        })),
    })
}

/// Upserts the user and issues a session token for them.
pub async fn create_session(
    db: &sqlx::PgPool,
    ttl: std::time::Duration,
    github_user: &GitHubUser,
    client_name: &str,
) -> Result<(String, chrono::DateTime<Utc>, User), Status> {
    let name = github_user.name.clone().unwrap_or_default();
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, github_id, github_login, name, avatar_url) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (github_id) DO UPDATE
           SET github_login = $3, name = $4, avatar_url = $5, last_login_at = now()
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(github_user.id)
    .bind(&github_user.login)
    .bind(&name)
    .bind(&github_user.avatar_url)
    .fetch_one(db)
    .await
    .map_err(internal)?;

    let token = new_token("blue_s_");
    let ttl = Duration::from_std(ttl).map_err(internal)?;
    let expires_at = Utc::now() + ttl;
    sqlx::query(
        "INSERT INTO user_sessions (id, user_id, token_hash, client_name, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(hash(&token).as_slice())
    .bind(client_name)
    .bind(expires_at)
    .execute(db)
    .await
    .map_err(internal)?;
    Ok((
        token,
        expires_at,
        User {
            github_id: github_user.id,
            github_login: github_user.login.clone(),
            name,
            avatar_url: github_user.avatar_url.clone(),
        },
    ))
}
