mod output;

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, bail};
use blue_common::Bearer;
use blue_common::credentials::CredentialStore;
use blue_protocol::auth::v1::auth_service_client::AuthServiceClient;
use blue_protocol::auth::v1::{
    CreateLiveKitTokenRequest, CreatePairingCodeRequest, GetCurrentUserRequest,
    LoginWithGitHubRequest, LogoutRequest, RevokeMachineRequest, login_with_git_hub_response,
};
use blue_protocol::client::v1::client_service_client::ClientServiceClient;
use blue_protocol::client::v1::{
    CreateSessionRequest, ListMachinesRequest, ListSessionsRequest, SendPromptRequest, Session,
    SubscribeRequest, event,
};
use blue_protocol::events::v1::session_event::Kind;
use clap::{Parser, Subcommand};
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;
use tonic_health::pb::{HealthCheckRequest, health_client::HealthClient};

type Client = ClientServiceClient<InterceptedService<Channel, Bearer>>;
type Auth = AuthServiceClient<InterceptedService<Channel, Bearer>>;

const TITLE_CHARS: usize = 40;

#[derive(Parser)]
#[command(name = "blue", version, about = "Text client for the Blue server")]
struct Cli {
    /// Server gRPC endpoint.
    #[arg(
        long,
        global = true,
        env = "BLUE_SERVER_URL",
        default_value = "http://127.0.0.1:50051"
    )]
    server: String,

    /// Session token to use instead of the stored one (or the voice service token).
    #[arg(long, global = true, env = "BLUE_TOKEN", hide_env_values = true)]
    token: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check that the server is up (gRPC health check).
    Health,
    /// Sign in with GitHub.
    Login,
    /// Sign out and forget the stored token.
    Logout,
    /// Show who you are signed in as.
    Whoami,
    /// Create a one-time code for pairing a machine (`blue-daemon pair`).
    PairCode,
    /// List machines, or revoke one.
    Machines {
        #[command(subcommand)]
        command: Option<MachinesCommand>,
    },
    /// List sessions.
    Sessions {
        /// Only sessions on this machine.
        #[arg(long)]
        machine: Option<String>,
        /// Include closed and failed sessions.
        #[arg(long)]
        all: bool,
    },
    /// Start a task on a machine and follow it until the first turn ends.
    Run {
        /// Machine id or name.
        machine: String,
        /// What Claude should do; say which repo or directory to work in.
        instructions: String,
        /// Short session name. Defaults to the start of the instructions.
        #[arg(long)]
        title: Option<String>,
    },
    /// Send a follow-up to a session and follow it until that turn ends.
    Send {
        /// Session id, id prefix, or title.
        session: String,
        instructions: String,
        /// Return once the turn is queued instead of following it.
        #[arg(long)]
        detach: bool,
    },
    /// Stream events as they happen.
    Tail {
        /// Only this session (id, id prefix, or title).
        #[arg(long)]
        session: Option<String>,
        /// Start from the beginning instead of only new events.
        #[arg(long)]
        from_start: bool,
        /// Include raw Agent SDK messages.
        #[arg(long)]
        raw: bool,
    },
    /// Print a LiveKit room token (for the Agents Playground).
    Token {
        #[arg(long)]
        room: Option<String>,
    },
}

#[derive(Subcommand)]
enum MachinesCommand {
    /// Revoke a machine's token and disconnect it.
    Revoke { machine: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let ctx = Ctx::new(cli.server, cli.token);
    match cli.command {
        Command::Health => health(&ctx).await,
        Command::Login => login(&ctx).await,
        Command::Logout => {
            let result = ctx.auth().await?.logout(LogoutRequest {}).await;
            ctx.store().delete()?;
            result?;
            println!("Signed out.");
            Ok(())
        }
        Command::Whoami => {
            let user = ctx
                .auth()
                .await?
                .get_current_user(GetCurrentUserRequest {})
                .await?
                .into_inner()
                .user
                .unwrap_or_default();
            println!("{} (GitHub id {})", user.github_login, user.github_id);
            Ok(())
        }
        Command::PairCode => {
            let code = ctx
                .auth()
                .await?
                .create_pairing_code(CreatePairingCodeRequest {})
                .await?
                .into_inner();
            println!("{}", code.code);
            eprintln!(
                "On the machine to pair (valid for 10 minutes):\n  blue-daemon pair --server {} --code {}",
                ctx.server, code.code
            );
            Ok(())
        }
        Command::Machines { command: None } => {
            let machines = ctx
                .client()
                .await?
                .list_machines(ListMachinesRequest {})
                .await?
                .into_inner()
                .machines;
            if machines.is_empty() {
                println!("No machines yet; pair one with `blue pair-code`.");
            }
            for machine in machines {
                let status = if machine.online { "online" } else { "offline" };
                println!(
                    "{:<20} {:<8} {:<8} {}",
                    machine.name, status, machine.os, machine.id
                );
            }
            Ok(())
        }
        Command::Machines {
            command: Some(MachinesCommand::Revoke { machine }),
        } => {
            ctx.auth()
                .await?
                .revoke_machine(RevokeMachineRequest {
                    machine: machine.clone(),
                })
                .await?;
            println!("Revoked {machine}.");
            Ok(())
        }
        Command::Sessions { machine, all } => {
            let sessions = ctx
                .client()
                .await?
                .list_sessions(ListSessionsRequest {
                    machine: machine.unwrap_or_default(),
                    active_only: !all,
                })
                .await?
                .into_inner()
                .sessions;
            if sessions.is_empty() {
                println!("No sessions.");
            }
            for session in sessions {
                println!("{}", output::session_line(&session));
            }
            Ok(())
        }
        Command::Run {
            machine,
            instructions,
            title,
        } => {
            let title = title.unwrap_or_else(|| default_title(&instructions));
            let mut client = ctx.client().await?;
            let created = client
                .create_session(CreateSessionRequest {
                    machine,
                    instructions,
                    title,
                })
                .await?
                .into_inner();
            let session = created.session.context("server returned no session")?;
            eprintln!(
                "Session {} on {}: {}",
                session.id, session.machine_name, session.title
            );
            follow(&mut client, &session.id, &created.turn_id).await
        }
        Command::Send {
            session,
            instructions,
            detach,
        } => {
            let mut client = ctx.client().await?;
            let session = find_session(&mut client, &session).await?;
            let turn_id = client
                .send_prompt(SendPromptRequest {
                    session_id: session.id.clone(),
                    instructions,
                })
                .await?
                .into_inner()
                .turn_id;
            if detach {
                println!("{turn_id}");
                return Ok(());
            }
            follow(&mut client, &session.id, &turn_id).await
        }
        Command::Tail {
            session,
            from_start,
            raw,
        } => {
            let mut client = ctx.client().await?;
            let session_ids = match session {
                Some(session) => vec![find_session(&mut client, &session).await?.id],
                None => Vec::new(),
            };
            tail(&mut client, session_ids, from_start, raw).await
        }
        Command::Token { room } => {
            let token = ctx
                .auth()
                .await?
                .create_live_kit_token(CreateLiveKitTokenRequest { room_name: room })
                .await?
                .into_inner();
            println!("{}", token.token);
            eprintln!(
                "LiveKit URL: {}\nRoom: {}",
                token.livekit_url, token.room_name
            );
            Ok(())
        }
    }
}

struct Ctx {
    server: String,
    token: Option<String>,
}

impl Ctx {
    fn new(server: String, token: Option<String>) -> Self {
        Self { server, token }
    }

    fn store(&self) -> CredentialStore {
        let safe: String = self
            .server
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let file = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("blue")
            .join(format!("cli-token-{safe}"));
        CredentialStore::new("blue", &self.server, file)
    }

    fn bearer(&self) -> anyhow::Result<Bearer> {
        let token = match &self.token {
            Some(token) => token.clone(),
            None => self
                .store()
                .get()?
                .context("not signed in; run `blue login`")?,
        };
        Bearer::new(&token)
    }

    async fn client(&self) -> anyhow::Result<Client> {
        let channel = blue_common::connect(&self.server).await?;
        Ok(ClientServiceClient::with_interceptor(
            channel,
            self.bearer()?,
        ))
    }

    async fn auth(&self) -> anyhow::Result<Auth> {
        let channel = blue_common::connect(&self.server).await?;
        Ok(AuthServiceClient::with_interceptor(channel, self.bearer()?))
    }
}

async fn health(ctx: &Ctx) -> anyhow::Result<()> {
    let channel = blue_common::connect(&ctx.server).await?;
    let response = HealthClient::new(channel)
        .check(HealthCheckRequest {
            service: String::new(),
        })
        .await?
        .into_inner();
    println!("{}: {}", ctx.server, response.status().as_str_name());
    Ok(())
}

async fn login(ctx: &Ctx) -> anyhow::Result<()> {
    let channel = blue_common::connect(&ctx.server).await?;
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let mut stream = AuthServiceClient::with_interceptor(channel, Bearer::none())
        .login_with_git_hub(LoginWithGitHubRequest {
            client_name: format!("blue CLI on {host}"),
        })
        .await?
        .into_inner();
    while let Some(response) = stream.message().await? {
        match response.step {
            Some(login_with_git_hub_response::Step::DeviceCode(code)) => {
                println!("Enter code {} at {}", code.user_code, code.verification_uri);
                let _ = webbrowser::open(&code.verification_uri);
            }
            Some(login_with_git_hub_response::Step::Complete(done)) => {
                ctx.store().set(&done.session_token)?;
                let login = done.user.map(|user| user.github_login).unwrap_or_default();
                println!("Signed in as {login}.");
                return Ok(());
            }
            None => {}
        }
    }
    bail!("the server ended sign-in without a result")
}

/// Prints a turn's events until it completes or fails.
async fn follow(client: &mut Client, session_id: &str, turn_id: &str) -> anyhow::Result<()> {
    let mut stream = client
        .subscribe(SubscribeRequest {
            cursor: Some(0),
            session_ids: vec![session_id.into()],
            include_raw: false,
        })
        .await?
        .into_inner();
    while let Some(response) = stream.message().await? {
        let Some(event::Kind::Session(envelope)) = response.event.and_then(|event| event.kind)
        else {
            continue;
        };
        if envelope.turn_id != turn_id {
            continue;
        }
        let Some(kind) = envelope.event.and_then(|event| event.kind) else {
            continue;
        };
        if let Some(line) = output::event_line(&kind) {
            println!("{line}");
        }
        match kind {
            Kind::TurnCompleted(_) => return Ok(()),
            Kind::TurnFailed(_) => std::process::exit(1),
            _ => {}
        }
    }
    bail!("the server closed the event stream")
}

async fn tail(
    client: &mut Client,
    session_ids: Vec<String>,
    from_start: bool,
    raw: bool,
) -> anyhow::Result<()> {
    let mut titles: HashMap<String, String> = HashMap::new();
    let mut stream = client
        .subscribe(SubscribeRequest {
            cursor: from_start.then_some(0),
            session_ids,
            include_raw: raw,
        })
        .await?
        .into_inner();
    while let Some(response) = stream.message().await? {
        match response.event.and_then(|event| event.kind) {
            Some(event::Kind::Session(envelope)) => {
                let Some(kind) = envelope.event.and_then(|event| event.kind) else {
                    continue;
                };
                let line = match &kind {
                    Kind::RawSdkMessage(message) => Some(message.json.clone()),
                    kind => output::event_line(kind),
                };
                let Some(line) = line else { continue };
                if !titles.contains_key(&envelope.session_id) {
                    titles = all_titles(client).await?;
                }
                let title = titles
                    .get(&envelope.session_id)
                    .map(String::as_str)
                    .unwrap_or(&envelope.session_id);
                println!("[{title}] {line}");
            }
            Some(event::Kind::MachineStatus(status)) => {
                let state = if status.online { "online" } else { "offline" };
                println!("machine {} is {state}", status.machine_name);
            }
            None => {}
        }
    }
    Ok(())
}

async fn all_titles(client: &mut Client) -> anyhow::Result<HashMap<String, String>> {
    let sessions = client
        .list_sessions(ListSessionsRequest {
            machine: String::new(),
            active_only: false,
        })
        .await?
        .into_inner()
        .sessions;
    Ok(sessions
        .into_iter()
        .map(|session| (session.id, session.title))
        .collect())
}

/// Finds a session by id, unique id prefix, or title (any case).
async fn find_session(client: &mut Client, query: &str) -> anyhow::Result<Session> {
    let sessions = client
        .list_sessions(ListSessionsRequest {
            machine: String::new(),
            active_only: false,
        })
        .await?
        .into_inner()
        .sessions;
    let matches: Vec<&Session> = sessions
        .iter()
        .filter(|session| session.id.starts_with(query))
        .collect();
    let matches = if matches.is_empty() {
        sessions
            .iter()
            .filter(|session| session.title.eq_ignore_ascii_case(query))
            .collect()
    } else {
        matches
    };
    match matches.as_slice() {
        [session] => Ok((*session).clone()),
        [] => bail!("no session matches {query:?}"),
        several => {
            let list: Vec<String> = several.iter().map(|s| output::session_line(s)).collect();
            bail!("{query:?} matches several sessions:\n{}", list.join("\n"))
        }
    }
}

fn default_title(instructions: &str) -> String {
    let first_line = instructions.lines().next().unwrap_or_default().trim();
    if first_line.chars().count() <= TITLE_CHARS {
        return first_line.to_owned();
    }
    let cut: String = first_line.chars().take(TITLE_CHARS).collect();
    match cut.rsplit_once(' ') {
        Some((head, _)) if !head.is_empty() => format!("{head}…"),
        _ => format!("{cut}…"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(
            default_title("add a health endpoint"),
            "add a health endpoint"
        );
        assert_eq!(
            default_title("in the blue repo, add a health endpoint that returns the version"),
            "in the blue repo, add a health endpoint…"
        );
    }
}
