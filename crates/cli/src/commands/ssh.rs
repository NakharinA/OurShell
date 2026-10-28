//! `connect`, `exec`, `sftp`, `forward`, `key` and `import` subcommands.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::Subcommand;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ourshell::client::{hello, AgentClient, CallError};
use ourshell_proto::agent::{
    event, method, ClientKind, DirEntry, ExecParams, ExecResult, FileData, ForwardInfo,
    ForwardKind, ForwardOpenParams, HostTarget, ImportFileParams, ItemListParams, ItemRecord,
    KeyGenerateParams, KeyImportParams, PtyDataEvent, PtyExitEvent, PtyOpenParams, PtyOpenResult,
    PtyResizeParams, PtyWriteParams, SessionRef, SftpPathParams, SftpWriteParams,
};
use ourshell_proto::rpc;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use super::{connect_unlocked, friendly, read_secret};

#[derive(Subcommand)]
pub enum SftpCmd {
    /// List a remote directory.
    Ls {
        host: String,
        #[arg(default_value = ".")]
        path: String,
    },
    /// Download a remote file.
    Get {
        host: String,
        remote: String,
        /// Local destination (default: the remote file name).
        local: Option<PathBuf>,
    },
    /// Upload a local file.
    Put {
        host: String,
        local: PathBuf,
        /// Remote destination (default: the local file name).
        remote: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum KeyCmd {
    /// Generate a new key pair and store it in the vault.
    Gen {
        name: String,
        /// ed25519 (default) or rsa.
        #[arg(short = 't', long, default_value = "ed25519")]
        algorithm: String,
        /// RSA key size (2048, 3072, 4096).
        #[arg(short, long)]
        bits: Option<usize>,
        /// Encrypt the private key with a passphrase (prompted).
        #[arg(long)]
        passphrase: bool,
    },
    /// Import an existing private key file.
    Import {
        name: String,
        file: PathBuf,
        /// The key is passphrase-protected (prompted).
        #[arg(long)]
        passphrase: bool,
    },
    /// List keys.
    #[command(alias = "list")]
    Ls,
    /// Print a key's public half (for authorized_keys).
    Pub { name: String },
}

#[derive(Subcommand)]
pub enum ImportCmd {
    /// Import hosts and keys from an OpenSSH config file (default ~/.ssh/config).
    SshConfig { file: Option<PathBuf> },
    /// Import hosts from a Termius-style CSV export.
    TermiusCsv { file: PathBuf },
}

#[derive(Subcommand)]
pub enum ForwardCmd {
    /// Forward a local port to a destination reachable from the host (-L).
    Local {
        host: String,
        /// Local bind address, e.g. 127.0.0.1:8080 or 8080.
        bind: String,
        /// Destination host:port as seen from the remote host.
        dest: String,
    },
    /// Forward a remote port back to a local destination (-R).
    Remote {
        host: String,
        /// Remote bind address, e.g. 0.0.0.0:9000 or 9000.
        bind: String,
        /// Local destination host:port.
        dest: String,
    },
    /// List active forwards in the agent.
    #[command(alias = "list")]
    Ls,
    /// Stop a forward by id.
    Stop { id: String },
}

/// Calls a host method, asking the user to trust an unknown host key and retrying.
async fn call_trusting<P, R>(
    c: &AgentClient,
    method: &str,
    params: &P,
    set_accept: impl Fn(&mut P, String),
) -> Result<R>
where
    P: Serialize + Clone,
    R: DeserializeOwned,
{
    match c.call::<P, R>(method, params).await {
        Ok(r) => Ok(r),
        Err(CallError::Rpc(e)) if e.code == rpc::HOST_KEY_UNKNOWN => {
            let data = e.data.clone().unwrap_or_default();
            let host = data["host_port"].as_str().unwrap_or("?");
            let fp = data["fingerprint"].as_str().unwrap_or("?");
            let key = data["key"].as_str().unwrap_or_default().to_string();
            let algo = key.split_whitespace().next().unwrap_or("");
            eprintln!("The authenticity of host '{host}' can't be established.");
            eprintln!("{algo} key fingerprint is {fp}.");
            eprint!("Are you sure you want to continue connecting (yes/no)? ");
            std::io::stderr().flush()?;
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            if line.trim() != "yes" {
                bail!("host key not trusted");
            }
            let mut p = params.clone();
            set_accept(&mut p, key);
            c.call::<P, R>(method, &p).await.map_err(friendly)
        }
        Err(e) => Err(friendly(e)),
    }
}

fn target(host: &str) -> HostTarget {
    HostTarget {
        host: host.into(),
        accept_host_key: None,
    }
}

pub async fn exec(
    host: String,
    command: Vec<String>,
    timeout: Option<u64>,
    json_out: bool,
) -> Result<()> {
    let c = connect_unlocked().await?;
    let p = ExecParams {
        target: target(&host),
        command: command.join(" "),
        timeout_secs: timeout,
        max_output: None,
    };
    let r: ExecResult = call_trusting(&c, method::EXEC_RUN, &p, |p, k| {
        p.target.accept_host_key = Some(k)
    })
    .await?;
    if json_out {
        println!("{}", serde_json::to_string(&r)?);
        return Ok(());
    }
    print!("{}", r.stdout);
    eprint!("{}", r.stderr);
    if r.truncated {
        eprintln!("\n[output truncated]");
    }
    std::process::exit(r.exit_code.map(|c| c as i32).unwrap_or(255));
}

pub async fn sftp(cmd: SftpCmd, json_out: bool) -> Result<()> {
    let c = connect_unlocked().await?;
    match cmd {
        SftpCmd::Ls { host, path } => {
            let p = SftpPathParams {
                target: target(&host),
                path,
                max_bytes: None,
            };
            let entries: Vec<DirEntry> = call_trusting(&c, method::SFTP_LIST, &p, |p, k| {
                p.target.accept_host_key = Some(k)
            })
            .await?;
            if json_out {
                println!("{}", serde_json::to_string(&entries)?);
                return Ok(());
            }
            for e in entries {
                let size = e.size.map(|s| s.to_string()).unwrap_or_default();
                println!(
                    "{} {:>12}  {}{}",
                    if e.is_dir { "d" } else { "-" },
                    size,
                    e.name,
                    if e.is_dir { "/" } else { "" }
                );
            }
        }
        SftpCmd::Get {
            host,
            remote,
            local,
        } => {
            let local = local.unwrap_or_else(|| {
                PathBuf::from(
                    remote
                        .rsplit('/')
                        .next()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("download"),
                )
            });
            let p = SftpPathParams {
                target: target(&host),
                path: remote.clone(),
                max_bytes: None,
            };
            let f: FileData = call_trusting(&c, method::SFTP_READ, &p, |p, k| {
                p.target.accept_host_key = Some(k)
            })
            .await?;
            std::fs::write(&local, &f.data)
                .with_context(|| format!("writing {}", local.display()))?;
            println!("{remote} -> {} ({} bytes)", local.display(), f.data.len());
        }
        SftpCmd::Put {
            host,
            local,
            remote,
        } => {
            let data =
                std::fs::read(&local).with_context(|| format!("reading {}", local.display()))?;
            let remote = remote.unwrap_or_else(|| {
                local
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "upload".into())
            });
            let len = data.len();
            let p = SftpWriteParams {
                target: target(&host),
                path: remote.clone(),
                data,
            };
            call_trusting::<_, Value>(&c, method::SFTP_WRITE, &p, |p, k| {
                p.target.accept_host_key = Some(k)
            })
            .await?;
            println!("{} -> {remote} ({len} bytes)", local.display());
        }
    }
    Ok(())
}

fn norm_bind(b: &str) -> String {
    if b.parse::<u16>().is_ok() {
        format!("127.0.0.1:{b}")
    } else {
        b.to_string()
    }
}

pub async fn forward(cmd: ForwardCmd, json_out: bool) -> Result<()> {
    let c = connect_unlocked().await?;
    let (host, kind, bind, dest) = match cmd {
        ForwardCmd::Ls => {
            let v: Vec<ForwardInfo> = c
                .call(method::FORWARD_LIST, &json!({}))
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{}", serde_json::to_string(&v)?);
            } else if v.is_empty() {
                println!("No active forwards.");
            } else {
                for f in v {
                    println!(
                        "{}  {:?}  {} -> {}  via {}",
                        f.id, f.kind, f.bind, f.dest, f.host
                    );
                }
            }
            return Ok(());
        }
        ForwardCmd::Stop { id } => {
            c.call::<_, Value>(method::FORWARD_CLOSE, &SessionRef { session_id: id })
                .await
                .map_err(friendly)?;
            println!("Forward stopped.");
            return Ok(());
        }
        ForwardCmd::Local { host, bind, dest } => {
            (host, ForwardKind::Local, norm_bind(&bind), dest)
        }
        ForwardCmd::Remote { host, bind, dest } => {
            let bind = if bind.parse::<u16>().is_ok() {
                format!("0.0.0.0:{bind}")
            } else {
                bind
            };
            (host, ForwardKind::Remote, bind, dest)
        }
    };
    let p = ForwardOpenParams {
        target: target(&host),
        kind,
        bind,
        dest,
    };
    let info: ForwardInfo = call_trusting(&c, method::FORWARD_OPEN, &p, |p, k| {
        p.target.accept_host_key = Some(k)
    })
    .await?;
    if json_out {
        println!("{}", serde_json::to_string(&info)?);
        return Ok(());
    }
    println!(
        "Forwarding {} -> {} via {} (id {}). Press Ctrl+C to stop.",
        info.bind, info.dest, info.host, info.id
    );
    tokio::signal::ctrl_c().await?;
    c.call::<_, Value>(
        method::FORWARD_CLOSE,
        &SessionRef {
            session_id: info.id,
        },
    )
    .await
    .map_err(friendly)?;
    println!("Forward stopped.");
    Ok(())
}

pub async fn key(cmd: KeyCmd, json_out: bool) -> Result<()> {
    let c = connect_unlocked().await?;
    match cmd {
        KeyCmd::Gen {
            name,
            algorithm,
            bits,
            passphrase,
        } => {
            let passphrase = if passphrase {
                Some(read_secret("Key passphrase: ")?)
            } else {
                None
            };
            let v: Value = c
                .call(
                    method::KEY_GENERATE,
                    &KeyGenerateParams {
                        name: name.clone(),
                        algorithm,
                        bits,
                        passphrase,
                    },
                )
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{v}");
            } else {
                println!(
                    "Generated key `{name}` ({}).",
                    v["fingerprint"].as_str().unwrap_or("")
                );
                println!("{}", v["public_key"].as_str().unwrap_or(""));
            }
        }
        KeyCmd::Import {
            name,
            file,
            passphrase,
        } => {
            let private_key = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let passphrase = if passphrase {
                Some(read_secret("Key passphrase: ")?)
            } else {
                None
            };
            let v: Value = c
                .call(
                    method::KEY_IMPORT,
                    &KeyImportParams {
                        name: name.clone(),
                        private_key,
                        passphrase,
                    },
                )
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{v}");
            } else {
                println!(
                    "Imported key `{name}` ({}).",
                    v["fingerprint"].as_str().unwrap_or("")
                );
            }
        }
        KeyCmd::Ls => {
            let keys: Vec<ItemRecord> = c
                .call(method::ITEM_LIST, &ItemListParams { kind: "key".into() })
                .await
                .map_err(friendly)?;
            if json_out {
                println!("{}", serde_json::to_string(&keys)?);
                return Ok(());
            }
            for k in keys {
                let public = k.item["public_key"].as_str().unwrap_or("");
                let algo = public.split_whitespace().next().unwrap_or("");
                println!("{:<20} {}", k.item["name"].as_str().unwrap_or(""), algo);
            }
        }
        KeyCmd::Pub { name } => {
            let k: ItemRecord = c
                .call(
                    method::ITEM_GET,
                    &ourshell_proto::agent::ItemRef {
                        kind: "key".into(),
                        id: name,
                    },
                )
                .await
                .map_err(friendly)?;
            println!("{}", k.item["public_key"].as_str().unwrap_or(""));
        }
    }
    Ok(())
}

pub async fn import(cmd: ImportCmd, json_out: bool) -> Result<()> {
    let c = connect_unlocked().await?;
    let (m, file) = match cmd {
        ImportCmd::SshConfig { file } => (
            method::IMPORT_SSH_CONFIG,
            file.unwrap_or_else(|| ourshell::ipc::home().join(".ssh").join("config")),
        ),
        ImportCmd::TermiusCsv { file } => (method::IMPORT_TERMIUS_CSV, file),
    };
    let contents =
        std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
    let r: Value = c
        .call(
            m,
            &ImportFileParams {
                contents,
                base_dir: file.parent().map(|p| p.display().to_string()),
            },
        )
        .await
        .map_err(friendly)?;
    if json_out {
        println!("{r}");
        return Ok(());
    }
    println!(
        "Imported {} hosts, {} keys, {} identities, {} groups.",
        r["hosts_added"], r["keys_added"], r["identities_added"], r["groups_added"]
    );
    if let Some(skipped) = r["hosts_skipped"].as_array().filter(|a| !a.is_empty()) {
        let names: Vec<&str> = skipped.iter().filter_map(Value::as_str).collect();
        println!("Skipped (already exist): {}", names.join(", "));
    }
    for w in r["warnings"].as_array().into_iter().flatten() {
        println!("warning: {}", w.as_str().unwrap_or(""));
    }
    Ok(())
}

/// Translates a key press into the bytes a terminal would send.
fn key_bytes(k: KeyEvent) -> Option<Vec<u8>> {
    if k.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let mut out = match k.code {
        KeyCode::Char(ch) if ctrl => {
            let c = ch.to_ascii_lowercase();
            match c {
                'a'..='z' => vec![c as u8 - b'a' + 1],
                '@' | ' ' | '2' => vec![0],
                '[' | '3' => vec![0x1b],
                '\\' | '4' => vec![0x1c],
                ']' | '5' => vec![0x1d],
                '^' | '6' => vec![0x1e],
                '_' | '7' | '/' => vec![0x1f],
                _ => ch.to_string().into_bytes(),
            }
        }
        KeyCode::Char(ch) => ch.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::F(n) => match n {
            1 => b"\x1bOP".to_vec(),
            2 => b"\x1bOQ".to_vec(),
            3 => b"\x1bOR".to_vec(),
            4 => b"\x1bOS".to_vec(),
            5 => b"\x1b[15~".to_vec(),
            6 => b"\x1b[17~".to_vec(),
            7 => b"\x1b[18~".to_vec(),
            8 => b"\x1b[19~".to_vec(),
            9 => b"\x1b[20~".to_vec(),
            10 => b"\x1b[21~".to_vec(),
            11 => b"\x1b[23~".to_vec(),
            12 => b"\x1b[24~".to_vec(),
            _ => return None,
        },
        _ => return None,
    };
    if alt {
        out.insert(0, 0x1b);
    }
    Some(out)
}

struct RawGuard;

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Interactive shell on a saved host.
pub async fn connect(host: String, agent_forwarding: bool) -> Result<()> {
    super::connect_unlocked().await?;
    let (c, mut events) = AgentClient::connect_or_spawn(hello(ClientKind::Cli)).await?;
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let p = PtyOpenParams {
        target: target(&host),
        term: std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()),
        cols: cols.into(),
        rows: rows.into(),
        agent_forwarding,
    };
    let opened: PtyOpenResult = call_trusting(&c, method::PTY_OPEN, &p, |p, k| {
        p.target.accept_host_key = Some(k)
    })
    .await?;
    let sid = opened.session_id;

    #[cfg(windows)]
    let _ = crossterm::ansi_support::supports_ansi();
    crossterm::terminal::enable_raw_mode()?;
    let _guard = RawGuard;
    let mut input = EventStream::new();
    let mut stdout = std::io::stdout();
    let exit_code = loop {
        tokio::select! {
            n = events.recv() => {
                let Some(n) = n else { break None };
                match n.method.as_str() {
                    event::PTY_DATA => {
                        if let Ok(d) = serde_json::from_value::<PtyDataEvent>(n.params) {
                            if d.session_id == sid {
                                stdout.write_all(&d.data)?;
                                stdout.flush()?;
                            }
                        }
                    }
                    event::PTY_EXIT => {
                        if let Ok(e) = serde_json::from_value::<PtyExitEvent>(n.params) {
                            if e.session_id == sid {
                                break e.exit_code;
                            }
                        }
                    }
                    _ => {}
                }
            }
            ev = input.next() => {
                match ev {
                    Some(Ok(Event::Key(k))) => {
                        if let Some(bytes) = key_bytes(k) {
                            let w = PtyWriteParams { session_id: sid.clone(), data: bytes };
                            if c.call::<_, Value>(method::PTY_WRITE, &w).await.is_err() {
                                break None;
                            }
                        }
                    }
                    Some(Ok(Event::Paste(text))) => {
                        let w = PtyWriteParams { session_id: sid.clone(), data: text.into_bytes() };
                        let _ = c.call::<_, Value>(method::PTY_WRITE, &w).await;
                    }
                    Some(Ok(Event::Resize(cols, rows))) => {
                        let r = PtyResizeParams { session_id: sid.clone(), cols: cols.into(), rows: rows.into() };
                        let _ = c.call::<_, Value>(method::PTY_RESIZE, &r).await;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(anyhow!("terminal input error: {e}")),
                    None => break None,
                }
            }
        }
    };
    drop(_guard);
    let _ = c
        .call::<_, Value>(method::PTY_CLOSE, &SessionRef { session_id: sid })
        .await;
    println!("\r\nConnection to {host} closed.");
    if let Some(code) = exit_code {
        if code != 0 {
            std::process::exit(code as i32);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_keys() {
        let k = |code, m| KeyEvent::new(code, m);
        assert_eq!(
            key_bytes(k(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(vec![3])
        );
        assert_eq!(
            key_bytes(k(KeyCode::Char('a'), KeyModifiers::NONE)),
            Some(b"a".to_vec())
        );
        assert_eq!(
            key_bytes(k(KeyCode::Up, KeyModifiers::NONE)),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            key_bytes(k(KeyCode::Char('x'), KeyModifiers::ALT)),
            Some(b"\x1bx".to_vec())
        );
    }
}
