//! Host key verification: the [`HostKeyVerifier`] trait, a few ready-made
//! implementations, and the [`OnUnknownHostKey`] policy callback used by
//! [`crate::ssh::connect`] to implement TOFU ("trust on first use").

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

/// The result of checking a presented host key against whatever the caller
/// already trusts for that host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKeyDecision {
    /// The key matches a record the caller already trusts.
    Trusted,
    /// No record exists for this host yet.
    Unknown,
    /// A record exists for this host, but the presented key is different.
    /// This is always refused; it is never routed through [`OnUnknownHostKey`].
    Mismatch,
}

/// Checks a presented host key against a trust store (a known_hosts file,
/// an in-memory list populated from the vault, or a combination of both via
/// [`ChainVerifier`]).
///
/// `host_port` is formatted as `"host:port"`. `key_openssh` is the
/// single-line OpenSSH public key format, e.g. `"ssh-ed25519 AAAA..."`
/// (no comment field).
pub trait HostKeyVerifier: Send + Sync {
    fn check(&self, host_port: &str, key_openssh: &str) -> HostKeyDecision;
}

/// A future returned by [`OnUnknownHostKey::decide`].
pub type UnknownHostFuture<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

/// Called by [`crate::ssh::connect`] when [`HostKeyVerifier::check`] returns
/// [`HostKeyDecision::Unknown`], so the caller can prompt the user (or
/// auto-accept, for tests/CI). Returning `true` accepts the key for this
/// connection; the caller is then responsible for persisting it (the
/// accepted key is also returned from `connect`).
///
/// A plain synchronous closure `Fn(&str, &str) -> bool` implements this
/// trait automatically. For an async decision (e.g. a UI prompt), implement
/// the trait directly and box your own future.
pub trait OnUnknownHostKey: Send + Sync {
    fn decide<'a>(&'a self, host_port: &'a str, key_openssh: &'a str) -> UnknownHostFuture<'a>;
}

impl<F> OnUnknownHostKey for F
where
    F: Fn(&str, &str) -> bool + Send + Sync,
{
    fn decide<'a>(&'a self, host_port: &'a str, key_openssh: &'a str) -> UnknownHostFuture<'a> {
        let accept = self(host_port, key_openssh);
        Box::pin(async move { accept })
    }
}

/// Always accepts unknown host keys. Useful for tests and non-interactive
/// tools; never use this for an interactive client.
pub struct AcceptAllUnknown;

impl OnUnknownHostKey for AcceptAllUnknown {
    fn decide<'a>(&'a self, _host_port: &'a str, _key_openssh: &'a str) -> UnknownHostFuture<'a> {
        Box::pin(async { true })
    }
}

/// Always rejects unknown host keys.
pub struct RejectAllUnknown;

impl OnUnknownHostKey for RejectAllUnknown {
    fn decide<'a>(&'a self, _host_port: &'a str, _key_openssh: &'a str) -> UnknownHostFuture<'a> {
        Box::pin(async { false })
    }
}

/// Splits a `"host:port"` string (as produced by [`host_port`]) into its
/// parts. Falls back to port 22 if there is no `:` (should not normally
/// happen, since callers always build these with [`host_port`]).
fn split_host_port(s: &str) -> (String, u16) {
    match s.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(22)),
        None => (s.to_string(), 22),
    }
}

/// Formats a `(host, port)` pair the same way everywhere in this module.
pub fn host_port(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

/// Reads (and can append to) an OpenSSH-style `known_hosts` file, such as
/// `~/.ssh/known_hosts`. Hashed (`|1|...`) entries are matched via
/// HMAC-SHA1, courtesy of `russh::keys::known_hosts`.
pub struct KnownHostsFile {
    path: PathBuf,
}

impl KnownHostsFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `~/.ssh/known_hosts`, resolved from `HOME` (or `USERPROFILE` on
    /// Windows).
    pub fn user_default() -> Option<Self> {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        Some(Self::new(
            PathBuf::from(home).join(".ssh").join("known_hosts"),
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends `key_openssh` for `host_port` to the file, creating parent
    /// directories as needed. Call this after `connect` reports a newly
    /// accepted key, if you want it remembered on disk as well as (or
    /// instead of) in the vault.
    pub fn learn(&self, host_port: &str, key_openssh: &str) -> std::io::Result<()> {
        let (host, port) = split_host_port(host_port);
        let pubkey = russh::keys::ssh_key::PublicKey::from_openssh(key_openssh)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        russh::keys::known_hosts::learn_known_hosts_path(&host, port, &pubkey, &self.path)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }
}

impl HostKeyVerifier for KnownHostsFile {
    fn check(&self, host_port: &str, key_openssh: &str) -> HostKeyDecision {
        let (host, port) = split_host_port(host_port);
        let Ok(presented) = russh::keys::ssh_key::PublicKey::from_openssh(key_openssh) else {
            return HostKeyDecision::Unknown;
        };
        match russh::keys::known_hosts::known_host_keys_path(&host, port, &self.path) {
            Ok(matches) if matches.is_empty() => HostKeyDecision::Unknown,
            Ok(matches) => {
                if matches.iter().any(|(_, k)| *k == presented) {
                    HostKeyDecision::Trusted
                } else {
                    HostKeyDecision::Mismatch
                }
            }
            Err(_) => HostKeyDecision::Unknown,
        }
    }
}

/// An in-memory known-hosts list, for callers (such as the vault) that keep
/// their own trusted-key records rather than an OpenSSH `known_hosts` file.
#[derive(Clone, Default)]
pub struct MemoryKnownHosts {
    entries: Vec<(String, String)>,
}

impl MemoryKnownHosts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_entries(entries: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
        }
    }

    pub fn add(&mut self, host_port: impl Into<String>, key_openssh: impl Into<String>) {
        self.entries.push((host_port.into(), key_openssh.into()));
    }
}

impl HostKeyVerifier for MemoryKnownHosts {
    fn check(&self, host_port: &str, key_openssh: &str) -> HostKeyDecision {
        let mut saw_host = false;
        for (hp, k) in &self.entries {
            if hp == host_port {
                saw_host = true;
                if k.trim() == key_openssh.trim() {
                    return HostKeyDecision::Trusted;
                }
            }
        }
        if saw_host {
            HostKeyDecision::Mismatch
        } else {
            HostKeyDecision::Unknown
        }
    }
}

/// Combines several verifiers, trying each in order and returning the first
/// non-[`Unknown`](HostKeyDecision::Unknown) decision. Falls back to
/// `Unknown` if every verifier does.
#[derive(Default)]
pub struct ChainVerifier(pub Vec<Box<dyn HostKeyVerifier>>);

impl ChainVerifier {
    pub fn new(verifiers: Vec<Box<dyn HostKeyVerifier>>) -> Self {
        Self(verifiers)
    }
}

impl HostKeyVerifier for ChainVerifier {
    fn check(&self, host_port: &str, key_openssh: &str) -> HostKeyDecision {
        for v in &self.0 {
            match v.check(host_port, key_openssh) {
                HostKeyDecision::Unknown => continue,
                decision => return decision,
            }
        }
        HostKeyDecision::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_known_hosts_trusted_unknown_mismatch() {
        let mut kh = MemoryKnownHosts::new();
        kh.add("example.com:22", "ssh-ed25519 AAAAexample");

        assert_eq!(
            kh.check("example.com:22", "ssh-ed25519 AAAAexample"),
            HostKeyDecision::Trusted
        );
        assert_eq!(
            kh.check("other.example.com:22", "ssh-ed25519 AAAAexample"),
            HostKeyDecision::Unknown
        );
        assert_eq!(
            kh.check("example.com:22", "ssh-ed25519 AAAAdifferent"),
            HostKeyDecision::Mismatch
        );
    }

    #[test]
    fn chain_verifier_tries_each_in_order() {
        let mut first = MemoryKnownHosts::new();
        first.add("a:22", "key-a");
        let mut second = MemoryKnownHosts::new();
        second.add("b:22", "key-b");

        let chain = ChainVerifier::new(vec![Box::new(first), Box::new(second)]);
        assert_eq!(chain.check("a:22", "key-a"), HostKeyDecision::Trusted);
        assert_eq!(chain.check("b:22", "key-b"), HostKeyDecision::Trusted);
        assert_eq!(chain.check("c:22", "key-c"), HostKeyDecision::Unknown);
    }

    #[test]
    fn known_hosts_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let kh = KnownHostsFile::new(&path);

        let key =
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
        assert_eq!(kh.check("localhost:2222", key), HostKeyDecision::Unknown);

        kh.learn("localhost:2222", key).unwrap();
        assert_eq!(kh.check("localhost:2222", key), HostKeyDecision::Trusted);

        let other =
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";
        assert_eq!(kh.check("localhost:2222", other), HostKeyDecision::Mismatch);
    }
}
