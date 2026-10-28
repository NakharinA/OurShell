//! Async JSON-RPC client for the local agent. Used by the CLI and the MCP server.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use interprocess::local_socket::tokio::{prelude::*, Stream};
use ourshell_proto::agent::{method, ClientKind, HelloParams, HelloResult};
use ourshell_proto::rpc::{Notification, Request, RpcError, ServerMessage};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, WriteHalf};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::ipc;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;

#[derive(Clone)]
pub struct AgentClient {
    writer: Arc<Mutex<WriteHalf<Stream>>>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
}

/// Error returned by [`AgentClient::call`]: either an RPC error from the agent or a transport error.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("{0}")]
    Rpc(RpcError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl CallError {
    pub fn code(&self) -> Option<i64> {
        match self {
            CallError::Rpc(e) => Some(e.code),
            CallError::Other(_) => None,
        }
    }
}

impl AgentClient {
    /// Connects to a running agent; does not spawn one.
    pub async fn connect_existing(
        hello: HelloParams,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Notification>)> {
        let id = ipc::socket_id();
        let stream = Stream::connect(ipc::socket_name(&id)?)
            .await
            .with_context(|| format!("agent not reachable at {id}"))?;
        let (r, w) = tokio::io::split(stream);
        let (ntx, nrx) = mpsc::unbounded_channel();
        let client = Self {
            writer: Arc::new(Mutex::new(w)),
            pending: Arc::default(),
            next_id: Arc::new(AtomicU64::new(1)),
        };
        let pending = client.pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<ServerMessage>(&line) {
                    Ok(ServerMessage::Response(resp)) => {
                        if let Some(tx) = pending.lock().await.remove(&resp.id) {
                            let _ = tx.send(match resp.error {
                                Some(e) => Err(e),
                                None => Ok(resp.result.unwrap_or(Value::Null)),
                            });
                        }
                    }
                    Ok(ServerMessage::Notification(n)) => {
                        let _ = ntx.send(n);
                    }
                    Err(_) => continue,
                }
            }
            // Connection closed: fail all waiters.
            for (_, tx) in pending.lock().await.drain() {
                let _ = tx.send(Err(RpcError::new(
                    ourshell_proto::rpc::INTERNAL_ERROR,
                    "agent disconnected",
                )));
            }
        });
        let res: HelloResult = client
            .call(method::HELLO, &hello)
            .await
            .map_err(|e| anyhow!("handshake failed: {e}"))?;
        if res.protocol != ourshell_proto::AGENT_PROTOCOL_VERSION {
            return Err(anyhow!(
                "agent speaks protocol v{}, this client v{}",
                res.protocol,
                ourshell_proto::AGENT_PROTOCOL_VERSION
            ));
        }
        Ok((client, nrx))
    }

    /// Connects, spawning `ourshell agent` in the background if needed.
    pub async fn connect_or_spawn(
        hello: HelloParams,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Notification>)> {
        if let Ok(c) = Self::connect_existing(hello.clone()).await {
            return Ok(c);
        }
        spawn_agent()?;
        let mut last = None;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match Self::connect_existing(hello.clone()).await {
                Ok(c) => return Ok(c),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("agent did not start")))
    }

    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R, CallError> {
        let v = self
            .call_value(
                method,
                serde_json::to_value(params).map_err(anyhow::Error::from)?,
            )
            .await?;
        serde_json::from_value(v).map_err(|e| CallError::Other(e.into()))
    }

    pub async fn call_value(&self, method: &str, params: Value) -> Result<Value, CallError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let mut line =
            serde_json::to_vec(&Request::new(id, method, params)).map_err(anyhow::Error::from)?;
        line.push(b'\n');
        {
            let mut w = self.writer.lock().await;
            w.write_all(&line).await.map_err(anyhow::Error::from)?;
            w.flush().await.map_err(anyhow::Error::from)?;
        }
        match rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(CallError::Rpc(e)),
            Err(_) => Err(CallError::Other(anyhow!("agent disconnected"))),
        }
    }
}

pub fn hello(client: ClientKind) -> HelloParams {
    HelloParams {
        protocol: ourshell_proto::AGENT_PROTOCOL_VERSION,
        client,
        ai_session: None,
        ai_client_name: None,
    }
}

/// Starts `ourshell agent` detached from this process.
fn spawn_agent() -> Result<()> {
    let exe = agent_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("agent")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    cmd.spawn().context("failed to start agent")?;
    Ok(())
}

/// The `ourshell` binary: env override, this executable if it is `ourshell`, a sibling, or PATH.
fn agent_exe() -> Result<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("OURSHELL_BIN") {
        return Ok(p.into());
    }
    let me = std::env::current_exe()?;
    let name = if cfg!(windows) {
        "ourshell.exe"
    } else {
        "ourshell"
    };
    if me.file_name().is_some_and(|n| n == name) {
        return Ok(me);
    }
    if let Some(sib) = me.parent().map(|d| d.join(name)).filter(|p| p.exists()) {
        return Ok(sib);
    }
    Ok(name.into())
}
