//! GitHub's OAuth device flow, run by the server on the client's behalf. The GitHub token is
//! used once, to read `/user`, and then dropped.

use std::time::Duration;

use anyhow::{Context, bail};
use serde::Deserialize;

use crate::config::GitHubConfig;

pub struct GitHub {
    http: reqwest::Client,
    client_id: String,
    web_url: String,
    api_url: String,
}

#[derive(Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Deserialize, Debug)]
pub struct GitHubUser {
    pub id: i64,
    pub login: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub avatar_url: String,
}

/// How a device-flow login ended, other than success.
#[derive(Debug)]
pub enum LoginError {
    Expired,
    Denied,
    Other(anyhow::Error),
}

impl From<anyhow::Error> for LoginError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

impl From<reqwest::Error> for LoginError {
    fn from(err: reqwest::Error) -> Self {
        Self::Other(err.into())
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    interval: Option<u64>,
}

impl GitHub {
    /// `None` when no client id is configured (sign-in disabled).
    pub fn new(config: &GitHubConfig) -> Option<Self> {
        Some(Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("blue-server/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(30))
                .build()
                .expect("HTTP client"),
            client_id: config.client_id.clone()?,
            web_url: config.web_url.trim_end_matches('/').to_owned(),
            api_url: config.api_url.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn device_code(&self) -> anyhow::Result<DeviceCode> {
        let response = self
            .http
            .post(format!("{}/login/device/code", self.web_url))
            .header("accept", "application/json")
            .form(&[("client_id", self.client_id.as_str())])
            .send()
            .await?
            .error_for_status()?;
        response
            .json()
            .await
            .context("unexpected device code response")
    }

    /// Polls until the user approves, then returns their GitHub account.
    pub async fn wait_for_user(&self, code: &DeviceCode) -> Result<GitHubUser, LoginError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(code.expires_in);
        let mut interval = Duration::from_secs(code.interval.max(1));
        loop {
            tokio::time::sleep(interval).await;
            if tokio::time::Instant::now() >= deadline {
                return Err(LoginError::Expired);
            }
            let response: TokenResponse = self
                .http
                .post(format!("{}/login/oauth/access_token", self.web_url))
                .header("accept", "application/json")
                .form(&[
                    ("client_id", self.client_id.as_str()),
                    ("device_code", code.device_code.as_str()),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ])
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            if let Some(token) = response.access_token {
                return Ok(self.user(&token).await?);
            }
            match response.error.as_deref() {
                Some("authorization_pending") => {}
                Some("slow_down") => {
                    interval = response
                        .interval
                        .map(Duration::from_secs)
                        .unwrap_or(interval + Duration::from_secs(5));
                }
                Some("expired_token") => return Err(LoginError::Expired),
                Some("access_denied") => return Err(LoginError::Denied),
                Some(other) => {
                    return Err(LoginError::Other(anyhow::anyhow!("GitHub error: {other}")));
                }
                None => return Err(LoginError::Other(anyhow::anyhow!("empty GitHub response"))),
            }
        }
    }

    async fn user(&self, token: &str) -> anyhow::Result<GitHubUser> {
        let response = self
            .http
            .get(format!("{}/user", self.api_url))
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("GitHub /user returned {}", response.status());
        }
        Ok(response.json().await?)
    }
}
