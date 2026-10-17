//! A running claude-bridge child process: protobuf frames over its stdin/stdout, stderr forwarded
//! to tracing, and the whole process tree (bridge + the `claude` it starts) contained so that
//! killing the bridge never leaves Claude running. Windows uses a Job Object that kills its
//! processes when closed; Unix uses a process group.

use std::collections::VecDeque;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail};
use blue_protocol::PROTOCOL_VERSION;
use blue_protocol::bridge::v1::{
    BridgeToDaemon, DaemonToBridge, Ready, bridge_to_daemon, daemon_to_bridge,
};
use blue_protocol::framing::{read_frame, write_frame};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;
use tokio::time::timeout;

const READY_TIMEOUT: Duration = Duration::from_secs(15);
const STDERR_TAIL_LINES: usize = 20;

pub struct BridgeProcess {
    child: Child,
    stdin: ChildStdin,
    events: mpsc::Receiver<anyhow::Result<bridge_to_daemon::Event>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    tree: ProcessTree,
}

impl BridgeProcess {
    /// Starts the bridge and waits for its `Ready` frame. With `replay`, the bridge plays back
    /// that recording instead of running Claude.
    pub async fn spawn(path: &Path, replay: Option<&Path>) -> anyhow::Result<(Self, Ready)> {
        let mut command = Command::new(path);
        if let Some(recording) = replay {
            command.env("BLUE_BRIDGE_REPLAY", recording);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .with_context(|| format!("spawn bridge {}", path.display()))?;
        let tree = ProcessTree::contain(&child)?;

        let stdin = child.stdin.take().context("bridge stdin")?;
        let stdout = child.stdout.take().context("bridge stdout")?;
        let stderr = child.stderr.take().context("bridge stderr")?;

        let (tx, events) = mpsc::channel(256);
        tokio::spawn(async move {
            let mut stdout = BufReader::new(stdout);
            loop {
                let event = match read_frame::<_, BridgeToDaemon>(&mut stdout).await {
                    Ok(Some(frame)) => match frame.event {
                        Some(event) => Ok(event),
                        None => continue,
                    },
                    Ok(None) => break,
                    Err(err) => Err(anyhow::Error::from(err).context("read bridge frame")),
                };
                let failed = event.is_err();
                if tx.send(event).await.is_err() || failed {
                    break;
                }
            }
        });

        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "bridge", "{line}");
                let mut tail = tail.lock().unwrap();
                if tail.len() == STDERR_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        });

        let mut bridge = Self {
            child,
            stdin,
            events,
            stderr_tail,
            tree,
        };
        let ready = match timeout(READY_TIMEOUT, bridge.next_event()).await {
            Err(_) => bail!("bridge sent no Ready within {READY_TIMEOUT:?}"),
            Ok(Some(Ok(bridge_to_daemon::Event::Ready(ready)))) => ready,
            Ok(Some(Ok(other))) => {
                bail!("expected Ready as the bridge's first frame, got {other:?}")
            }
            Ok(Some(Err(err))) => return Err(err),
            Ok(None) => bail!("bridge exited before Ready: {}", bridge.stderr_tail()),
        };
        if ready.protocol_version != PROTOCOL_VERSION {
            bail!(
                "bridge speaks protocol v{}, daemon speaks v{PROTOCOL_VERSION}",
                ready.protocol_version
            );
        }
        Ok((bridge, ready))
    }

    pub async fn send(&mut self, command: daemon_to_bridge::Command) -> anyhow::Result<()> {
        let frame = DaemonToBridge {
            command: Some(command),
        };
        write_frame(&mut self.stdin, &frame)
            .await
            .context("write to bridge")
    }

    /// The next frame from the bridge; `None` once its stdout has closed. Cancel-safe.
    pub async fn next_event(&mut self) -> Option<anyhow::Result<bridge_to_daemon::Event>> {
        self.events.recv().await
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// The last lines the bridge wrote to stderr, for error reports.
    pub fn stderr_tail(&self) -> String {
        let tail = self.stderr_tail.lock().unwrap();
        tail.iter().cloned().collect::<Vec<_>>().join("\n")
    }

    pub async fn wait(&mut self) -> anyhow::Result<ExitStatus> {
        Ok(self.child.wait().await?)
    }

    /// Kills the bridge and everything it started.
    pub fn kill_tree(&mut self) {
        self.tree.kill();
        let _ = self.child.start_kill();
    }
}

impl Drop for BridgeProcess {
    fn drop(&mut self) {
        self.kill_tree();
    }
}

#[cfg(windows)]
struct ProcessTree(Option<win32job::Job>);

#[cfg(windows)]
impl ProcessTree {
    fn contain(child: &Child) -> anyhow::Result<Self> {
        let job = win32job::Job::create().context("create job object")?;
        let mut limits = win32job::ExtendedLimitInfo::new();
        limits.limit_kill_on_job_close();
        job.set_extended_limit_info(&limits)
            .context("configure job object")?;
        let handle = child.raw_handle().context("bridge process handle")?;
        // The bridge only starts `claude` after it receives Start, which is sent after this
        // returns, so every descendant is created inside the job.
        job.assign_process(handle as isize)
            .context("assign bridge to job object")?;
        Ok(Self(Some(job)))
    }

    fn kill(&mut self) {
        // Closing the job handle kills every process in it (JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE).
        self.0.take();
    }
}

#[cfg(unix)]
struct ProcessTree(Option<nix::unistd::Pid>);

#[cfg(unix)]
impl ProcessTree {
    fn contain(child: &Child) -> anyhow::Result<Self> {
        let pid = child.id().context("bridge pid")?;
        // spawn() put the bridge in its own process group (process_group(0)), whose id is its pid.
        Ok(Self(Some(nix::unistd::Pid::from_raw(pid as i32))))
    }

    fn kill(&mut self) {
        if let Some(group) = self.0.take() {
            let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
        }
    }
}
