//! Optional built-in HTTPS with a Let's Encrypt certificate.
//!
//! When the server is set up and has a TLS domain (`OURSHELL_TLS_DOMAIN` or
//! the admin page), every serve generation in [`crate::run`] also serves
//! the app over HTTPS on `OURSHELL_TLS_BIND` (default `0.0.0.0:8444`; the
//! installer maps host port 443 to it). Plain HTTP on `BIND_ADDR` keeps
//! running regardless, for LAN access, the setup wizard and the healthcheck.
//!
//! Certificates come from Let's Encrypt via the TLS-ALPN-01 challenge (so
//! no port 80 is needed) and are cached in `<data dir>/acme`. Everything
//! uses rustls' `ring` provider, never aws-lc-rs.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use rustls_acme::caches::DirCache;
use rustls_acme::{is_tls_alpn_challenge, AcmeConfig, EventOk};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_rustls::rustls::server::Acceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::server::TlsStream;
use tokio_rustls::LazyConfigAcceptor;

use crate::config::{Config, TlsConfig};

pub const DEFAULT_TLS_BIND: &str = "0.0.0.0:8444";
pub const TLS_BIND_ENV: &str = "OURSHELL_TLS_BIND";
pub const ACME_STAGING_ENV: &str = "OURSHELL_ACME_STAGING";
/// Subdirectory of the data dir holding the ACME account and certificates.
pub const ACME_CACHE_DIR: &str = "acme";

/// A slow or silent client gets this long to finish its TLS handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the HTTPS listener binds: `OURSHELL_TLS_BIND` (read through `env`,
/// so tests needn't touch the process environment), else
/// [`DEFAULT_TLS_BIND`].
pub fn bind_addr(env: &dyn Fn(&str) -> Option<String>) -> Result<SocketAddr> {
    let raw = env(TLS_BIND_ENV)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_TLS_BIND.to_string());
    raw.parse()
        .with_context(|| format!("{TLS_BIND_ENV} {raw:?} is not a valid socket address"))
}

/// Whether to use the Let's Encrypt *staging* directory (untrusted test
/// certificates, generous rate limits): `OURSHELL_ACME_STAGING` set to
/// 1/true/yes/on. Anything else means production.
pub fn acme_staging(env: &dyn Fn(&str) -> Option<String>) -> bool {
    env(ACME_STAGING_ENV).is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// The TLS settings a serve generation should use: only once the server is
/// set up (`config` is `Some`) and a non-empty domain is configured.
pub fn wanted(config: Option<&Config>) -> Option<TlsConfig> {
    config
        .and_then(|c| c.tls.clone())
        .filter(|t| !t.domain.trim().is_empty())
}

/// Marks requests that arrived over the HTTPS listener (a request
/// extension added by a layer on the TLS app), so handlers can tell they
/// were served over TLS, e.g. to set `Secure` cookies.
#[derive(Clone, Copy, Debug)]
pub struct ViaTls;

/// Human-readable state of the built-in HTTPS listener for the admin page:
/// `None` when it is off, else e.g. "active" or "waiting for certificate".
/// Shared across reload generations.
#[derive(Default)]
pub struct TlsStatus(Mutex<Option<String>>);

impl TlsStatus {
    pub fn get(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }

    fn set(&self, status: Option<String>) {
        *self.0.lock().unwrap() = status;
    }
}

/// The HTTPS socket: bound lazily, the first time a generation needs it,
/// then shared by every later generation (like `SharedListener` for plain
/// HTTP; duplicating socket handles doesn't work on Windows).
pub struct TlsSocket {
    addr: SocketAddr,
    listener: Option<Arc<TcpListener>>,
}

impl TlsSocket {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            listener: None,
        }
    }

    async fn get(&mut self) -> Result<Arc<TcpListener>> {
        if let Some(l) = &self.listener {
            return Ok(l.clone());
        }
        let l = Arc::new(
            TcpListener::bind(self.addr)
                .await
                .with_context(|| format!("failed to listen on {} for HTTPS", self.addr))?,
        );
        tracing::info!(addr = %self.addr, "HTTPS listener bound");
        self.listener = Some(l.clone());
        Ok(l)
    }
}

/// One serve generation's HTTPS side. Dropping it (when the generation
/// ends on reload) stops its ACME renewal and accept tasks.
pub struct Generation {
    /// The listener to serve the app on; `None` when this generation serves
    /// no HTTPS.
    pub listener: Option<TlsListener>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Generation {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Sets up a generation's HTTPS side for `tls` (from [`wanted`]). Never
/// fails: if the socket can't be bound, the error is logged and shown on
/// the admin page, and the server keeps serving plain HTTP.
pub async fn start(
    socket: &mut TlsSocket,
    tls: Option<&TlsConfig>,
    data_dir: &Path,
    staging: bool,
    status: &Arc<TlsStatus>,
) -> Generation {
    let Some(tls) = tls else {
        status.set(None);
        // HTTPS was on in an earlier generation: refuse connections instead
        // of leaving them hanging in the backlog.
        let tasks = socket
            .listener
            .clone()
            .map(|l| tokio::spawn(refuse_loop(l)))
            .into_iter()
            .collect();
        return Generation {
            listener: None,
            tasks,
        };
    };
    let tcp = match socket.get().await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "HTTPS disabled");
            status.set(Some(format!("error: {e:#}")));
            return Generation {
                listener: None,
                tasks: Vec::new(),
            };
        }
    };
    let local_addr = tcp.local_addr().unwrap_or(socket.addr);
    tracing::info!(
        domain = %tls.domain,
        addr = %local_addr,
        staging,
        "serving HTTPS with a Let's Encrypt certificate (TLS-ALPN-01)"
    );
    status.set(Some("waiting for certificate".into()));

    let mut state = AcmeConfig::new([tls.domain.clone()])
        .contact(tls.email.iter().map(|e| format!("mailto:{e}")))
        .cache(DirCache::new(data_dir.join(ACME_CACHE_DIR)))
        .directory_lets_encrypt(!staging)
        .state();
    let challenge = state.challenge_rustls_config();
    let mut default = (*state.default_rustls_config()).clone();
    // axum serves HTTP/1.1 and HTTP/2 on the same connection.
    default.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let default = Arc::new(default);

    // Drives certificate loading, ordering and renewal for as long as this
    // generation runs.
    let acme_status = status.clone();
    let acme = tokio::spawn(async move {
        let mut have_cert = false;
        while let Some(event) = state.next().await {
            match event {
                Ok(ok) => {
                    match ok {
                        EventOk::DeployedCachedCert => {
                            tracing::info!("ACME: using the cached certificate")
                        }
                        EventOk::DeployedNewCert => {
                            tracing::info!("ACME: obtained a new certificate")
                        }
                        EventOk::CertCacheStore => tracing::debug!("ACME: certificate cached"),
                        EventOk::AccountCacheStore => tracing::debug!("ACME: account cached"),
                    }
                    if matches!(ok, EventOk::DeployedCachedCert | EventOk::DeployedNewCert) {
                        have_cert = true;
                        acme_status.set(Some("active".into()));
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "ACME: certificate error (will retry)");
                    acme_status.set(Some(if have_cert {
                        format!("active (renewal failed: {e})")
                    } else {
                        format!("waiting for certificate (last error: {e})")
                    }));
                }
            }
        }
    });

    let (tx, rx) = mpsc::channel(64);
    let accept = tokio::spawn(accept_loop(tcp, challenge, default, tx));
    Generation {
        listener: Some(TlsListener { rx, local_addr }),
        tasks: vec![acme, accept],
    }
}

/// Accepts TCP connections and runs each TLS handshake in its own task, so
/// one slow client can't hold up the others. Finished connections go to the
/// generation's [`TlsListener`]; ACME validation connections are answered
/// here and closed.
async fn accept_loop(
    tcp: Arc<TcpListener>,
    challenge: Arc<ServerConfig>,
    default: Arc<ServerConfig>,
    tx: mpsc::Sender<(TlsStream<TcpStream>, SocketAddr)>,
) {
    loop {
        let (stream, peer) = match tcp.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!(error = %e, "HTTPS accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let (challenge, default, tx) = (challenge.clone(), default.clone(), tx.clone());
        tokio::spawn(async move {
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(stream, challenge, default))
                .await
            {
                Ok(Ok(Some(tls))) => {
                    // Fails only if the generation ended meanwhile.
                    let _ = tx.send((tls, peer)).await;
                }
                Ok(Ok(None)) => {}
                Ok(Err(e)) => tracing::debug!(%peer, error = %e, "TLS handshake failed"),
                Err(_) => tracing::debug!(%peer, "TLS handshake timed out"),
            }
        });
    }
}

/// `None` for an ACME TLS-ALPN-01 validation connection (answered here).
async fn handshake(
    stream: TcpStream,
    challenge: Arc<ServerConfig>,
    default: Arc<ServerConfig>,
) -> std::io::Result<Option<TlsStream<TcpStream>>> {
    let start = LazyConfigAcceptor::new(Acceptor::default(), stream).await?;
    if is_tls_alpn_challenge(&start.client_hello()) {
        tracing::info!("ACME: answering a TLS-ALPN-01 validation request");
        let mut tls = start.into_stream(challenge).await?;
        tls.shutdown().await?;
        return Ok(None);
    }
    Ok(Some(start.into_stream(default).await?))
}

/// Closes every connection right away (HTTPS turned off by a reload).
async fn refuse_loop(tcp: Arc<TcpListener>) {
    loop {
        if tcp.accept().await.is_err() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// The HTTPS side of one generation as an [`axum::serve::Listener`]:
/// yields connections whose TLS handshake already completed.
pub struct TlsListener {
    rx: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    local_addr: SocketAddr,
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(conn) => conn,
            // The accept task is gone (generation ending): wait for the
            // graceful shutdown signal.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local_addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{apply_env, Effective, FileConfig, FileTls};
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn tls_bind_defaults_and_parses() {
        assert_eq!(
            bind_addr(&env_of(&[])).unwrap(),
            DEFAULT_TLS_BIND.parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            bind_addr(&env_of(&[(TLS_BIND_ENV, "  ")])).unwrap().port(),
            8444
        );
        assert_eq!(
            bind_addr(&env_of(&[(TLS_BIND_ENV, "127.0.0.1:9443")])).unwrap(),
            "127.0.0.1:9443".parse::<SocketAddr>().unwrap()
        );
        assert!(bind_addr(&env_of(&[(TLS_BIND_ENV, "443")])).is_err());
    }

    #[test]
    fn staging_only_when_truthy() {
        assert!(!acme_staging(&env_of(&[])));
        for v in ["1", "true", "YES", " on "] {
            assert!(acme_staging(&env_of(&[(ACME_STAGING_ENV, v)])), "{v}");
        }
        for v in ["", "0", "false", "off", "staging"] {
            assert!(!acme_staging(&env_of(&[(ACME_STAGING_ENV, v)])), "{v}");
        }
    }

    fn ready() -> Effective {
        let mut eff = Effective::default();
        eff.values.database_url = Some("postgres://x".into());
        eff.values.server_master_key = Some(crate::config::generate_master_key());
        eff
    }

    #[test]
    fn tls_served_only_when_ready_with_a_domain() {
        // Setup mode: never, even with a domain.
        let setup = apply_env(
            &FileConfig::default(),
            &env_of(&[("OURSHELL_TLS_DOMAIN", "sync.example.com")]),
        )
        .unwrap();
        let cfg = Config::from_effective(&setup).unwrap();
        assert!(cfg.is_none());
        assert_eq!(wanted(cfg.as_ref()), None);

        // Ready, no domain.
        let mut eff = ready();
        assert_eq!(wanted(Config::from_effective(&eff).unwrap().as_ref()), None);

        // Ready, empty domain.
        eff.values.tls = Some(FileTls {
            domain: String::new(),
            email: Some("a@b.c".into()),
        });
        assert_eq!(wanted(Config::from_effective(&eff).unwrap().as_ref()), None);

        // Ready with a domain.
        eff.values.tls = Some(FileTls {
            domain: "sync.example.com".into(),
            email: Some(String::new()),
        });
        let tls = wanted(Config::from_effective(&eff).unwrap().as_ref()).unwrap();
        assert_eq!(tls.domain, "sync.example.com");
        assert_eq!(tls.email, None);
    }

    #[test]
    fn status_tracks_last_value() {
        let s = TlsStatus::default();
        assert_eq!(s.get(), None);
        s.set(Some("active".into()));
        assert_eq!(s.get().as_deref(), Some("active"));
    }

    #[tokio::test]
    async fn no_tls_binds_nothing() {
        let mut socket = TlsSocket::new("127.0.0.1:0".parse().unwrap());
        let status = Arc::new(TlsStatus::default());
        let generation = start(&mut socket, None, Path::new("."), true, &status).await;
        assert!(generation.listener.is_none());
        assert!(socket.listener.is_none());
        assert_eq!(status.get(), None);
    }

    #[tokio::test]
    async fn tls_binds_once_and_is_reused() {
        let mut socket = TlsSocket::new("127.0.0.1:0".parse().unwrap());
        let status = Arc::new(TlsStatus::default());
        let tls = TlsConfig {
            domain: "example.invalid".into(),
            email: None,
        };
        // Never written to: the ACME task is dropped long before an
        // account could be created.
        let dir = std::env::temp_dir().join(format!("ourshell-tls-{}", uuid::Uuid::new_v4()));
        let first = start(&mut socket, Some(&tls), &dir, true, &status).await;
        let addr = first.listener.as_ref().unwrap().local_addr;
        assert_ne!(addr.port(), 0);
        assert!(status.get().unwrap().starts_with("waiting for certificate"));
        drop(first);
        // A later generation reuses the same socket rather than rebinding.
        let second = start(&mut socket, Some(&tls), &dir, true, &status).await;
        assert_eq!(second.listener.as_ref().unwrap().local_addr, addr);
        drop(second);
        let off = start(&mut socket, None, &dir, true, &status).await;
        assert!(off.listener.is_none());
        assert_eq!(status.get(), None);
    }
}
