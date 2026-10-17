//! blue-daemon-test: drives the daemon's session API from the terminal, for local testing
//! without a server. Not shipped; the daemon itself only exposes `doctor` and, later, `start`.

use std::path::PathBuf;

use anyhow::Context;
use blue_daemon::claude;
use blue_daemon::doctor::ExecutablePaths;
use blue_daemon::recorder::Recorder;
use blue_daemon::session::{Session, SessionConfig};
use blue_protocol::events::v1::{SessionState, session_event::Kind};
use clap::{Parser, Subcommand};
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "blue-daemon-test",
    version,
    about = "Local testing tool for blue-daemon sessions"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a Claude session in this terminal, without a server. Each line you type is a prompt;
    /// `/quit` ends the session once the running turn finishes.
    Session {
        #[command(flatten)]
        paths: ExecutablePaths,
        /// Where Claude looks for the project (or, with --resume, the session's own directory).
        /// Defaults to the home directory.
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Claude session id to resume.
        #[arg(long, requires = "cwd")]
        resume: Option<String>,
        /// Claude model, e.g. `haiku` or `sonnet`. Defaults to Claude Code's default.
        #[arg(long)]
        model: Option<String>,
        /// Record raw SDK messages (scrubbed) to this JSON-lines file.
        #[arg(long)]
        record: Option<PathBuf>,
        /// Play back a recording instead of running Claude: each prompt replays the next
        /// recorded turn.
        #[arg(long, conflicts_with = "record")]
        replay: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    match Cli::parse().command {
        Command::Session {
            paths,
            cwd,
            resume,
            model,
            record,
            replay,
        } => {
            // Replay never runs Claude, so it doesn't need it installed.
            let claude_path = match &replay {
                Some(_) => paths.claude.clone().unwrap_or_else(|| "claude".into()),
                None => claude::resolve(paths.claude.as_deref()).await?.path,
            };
            let cwd = match cwd {
                Some(cwd) => std::path::absolute(cwd)?,
                None => std::env::home_dir().context("no home directory")?,
            };
            let mut session = Session::start(SessionConfig {
                session_id: "local".into(),
                bridge_path: paths.bridge()?,
                claude_path,
                cwd,
                resume_claude_session_id: resume,
                model,
                replay,
            })
            .await?;
            if let Some(path) = record {
                session.record_to(Recorder::create(&path)?);
            }
            interactive(session).await
        }
    }
}

async fn interactive(mut session: Session) -> anyhow::Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut turns = 0;
    let mut stdin_open = true;
    loop {
        tokio::select! {
            line = lines.next_line(), if stdin_open => match line?.as_deref().map(str::trim) {
                None | Some("/quit") => {
                    stdin_open = false;
                    session.close().await?;
                }
                Some("") => {}
                Some(_) if session.turn_running() => {
                    println!("(a turn is still running; wait for it to finish)");
                }
                Some(text) => {
                    turns += 1;
                    session.prompt(format!("turn-{turns}"), text.to_owned()).await?;
                }
            },
            event = session.next() => {
                let Some(event) = event else { break };
                let Some(kind) = event.into_kind() else { continue };
                print_event(kind);
            }
        }
    }
    if let Some(id) = session.claude_session_id() {
        println!(
            "Resume with: blue-daemon-test session --resume {id} --cwd \"{}\"",
            session.cwd()
        );
    }
    Ok(())
}

fn print_event(kind: Kind) {
    match kind {
        Kind::SessionStarted(started) => {
            println!("● Claude ({}) working in {}", started.model, started.cwd)
        }
        Kind::AssistantText(text) => println!("{}", text.text),
        Kind::ToolStarted(tool) => println!("  → {}", tool.summary),
        Kind::ToolFinished(tool) if tool.is_error => println!("  ✗ tool call failed"),
        Kind::TurnCompleted(done) => {
            println!("✓ done in {:.1}s", done.duration_ms as f64 / 1000.0)
        }
        Kind::TurnFailed(failed) => println!("✗ turn failed: {}", failed.reason),
        Kind::SessionStateChanged(changed) => match changed.state() {
            SessionState::Failed => println!("✗ session failed"),
            SessionState::Closed => println!("● session closed"),
            _ => {}
        },
        _ => {}
    }
}
