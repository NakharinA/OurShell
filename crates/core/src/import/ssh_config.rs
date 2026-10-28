//! Hand-rolled parser and importer for OpenSSH `~/.ssh/config` files.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use super::{ImportOptions, ImportReport, Result};
use crate::vault::{Host, Identity, SshKey, Vault};

/// One `Host` block resolved from the config (wildcard-only blocks like
/// `Host *` are not emitted here; their settings are folded into later
/// concrete hosts as defaults instead).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub identity_files: Vec<String>,
    /// The `ProxyJump` value exactly as written, which may be a comma
    /// separated list of hops (`jump1,jump2`).
    pub proxy_jump: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct Fields {
    hostname: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    identity_files: Vec<String>,
    proxy_jump: Option<String>,
}

impl Fields {
    /// Fills any field this block didn't set from the accumulated defaults.
    fn fill_missing_from(&mut self, defaults: &Fields) {
        if self.hostname.is_none() {
            self.hostname = defaults.hostname.clone();
        }
        if self.port.is_none() {
            self.port = defaults.port;
        }
        if self.user.is_none() {
            self.user = defaults.user.clone();
        }
        if self.proxy_jump.is_none() {
            self.proxy_jump = defaults.proxy_jump.clone();
        }
        if self.identity_files.is_empty() {
            self.identity_files = defaults.identity_files.clone();
        }
    }

    /// Merges a wildcard block's fields into this (the running defaults),
    /// first-occurrence-wins for scalars, cumulative for `IdentityFile`.
    fn merge_as_default(&mut self, other: &Fields) {
        if self.hostname.is_none() {
            self.hostname = other.hostname.clone();
        }
        if self.port.is_none() {
            self.port = other.port;
        }
        if self.user.is_none() {
            self.user = other.user.clone();
        }
        if self.proxy_jump.is_none() {
            self.proxy_jump = other.proxy_jump.clone();
        }
        for f in &other.identity_files {
            if !self.identity_files.contains(f) {
                self.identity_files.push(f.clone());
            }
        }
    }
}

/// Splits a config line into `(key, raw_value)`, or `None` for a blank line
/// or a comment (a line whose first non-whitespace character is `#`).
fn split_key_value(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let key_end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let key = line[..key_end].to_string();
    let rest = line[key_end..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
    Some((key, rest.to_string()))
}

/// Splits a value into whitespace-separated words, treating `"..."` as one
/// word even if it contains whitespace.
fn split_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut has_cur = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_cur = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_cur {
                    out.push(std::mem::take(&mut cur));
                    has_cur = false;
                }
            }
            c => {
                cur.push(c);
                has_cur = true;
            }
        }
    }
    if has_cur {
        out.push(cur);
    }
    out
}

/// Un-quotes and joins a value into a single string (quoted whitespace is
/// preserved as part of the value).
fn value_of(raw: &str) -> String {
    split_words(raw).join(" ")
}

fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        home_dir().join(rest)
    } else if p == "~" {
        home_dir()
    } else {
        PathBuf::from(p)
    }
}

/// Parses already-flattened config lines (i.e. with `Include` already
/// expanded in place, or simply none present) into [`ParsedHost`]s.
pub fn parse_flat_lines(lines: &[String]) -> Vec<ParsedHost> {
    let mut results = Vec::new();
    let mut defaults = Fields::default();
    let mut current: Option<(Vec<String>, Fields)> = None;

    fn finalize(
        block: (Vec<String>, Fields),
        defaults: &mut Fields,
        results: &mut Vec<ParsedHost>,
    ) {
        let (aliases, fields) = block;
        if aliases.is_empty() {
            // Wildcard-only block: becomes a default for later hosts.
            defaults.merge_as_default(&fields);
            return;
        }
        let mut fields = fields;
        fields.fill_missing_from(defaults);
        for alias in aliases {
            results.push(ParsedHost {
                alias,
                hostname: fields.hostname.clone(),
                port: fields.port,
                user: fields.user.clone(),
                identity_files: fields.identity_files.clone(),
                proxy_jump: fields.proxy_jump.clone(),
            });
        }
    }

    for raw in lines {
        let Some((key, value)) = split_key_value(raw) else {
            continue;
        };
        if key.eq_ignore_ascii_case("host") {
            if let Some(block) = current.take() {
                finalize(block, &mut defaults, &mut results);
            }
            let patterns = split_words(&value);
            let aliases: Vec<String> = patterns.into_iter().filter(|p| p != "*").collect();
            current = Some((aliases, Fields::default()));
            continue;
        }
        let Some((_, fields)) = current.as_mut() else {
            // Directive before any `Host` line: not supported, ignore.
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "hostname" => fields.hostname = Some(value_of(&value)),
            "user" => fields.user = Some(value_of(&value)),
            "port" => fields.port = value_of(&value).parse().ok(),
            "identityfile" => {
                let path = expand_tilde(&value_of(&value));
                fields
                    .identity_files
                    .push(path.to_string_lossy().into_owned());
            }
            "proxyjump" => fields.proxy_jump = Some(value_of(&value)),
            _ => {}
        }
    }
    if let Some(block) = current.take() {
        finalize(block, &mut defaults, &mut results);
    }
    results
}

/// Parses a config given directly as text, with no `Include` support (use
/// [`parse_config`] to read a real file and follow its includes).
pub fn parse_text(text: &str) -> Vec<ParsedHost> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    parse_flat_lines(&lines)
}

const MAX_INCLUDE_DEPTH: u32 = 8;

fn glob_match(pattern: &str, text: &str) -> bool {
    fn helper(p: &[char], t: &[char]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some('*'), _) => helper(&p[1..], t) || (!t.is_empty() && helper(p, &t[1..])),
            (Some('?'), Some(_)) => helper(&p[1..], &t[1..]),
            (Some(pc), Some(tc)) if pc == tc => helper(&p[1..], &t[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    helper(&p, &t)
}

fn has_wildcard(p: &Path) -> bool {
    p.to_str()
        .map(|s| s.contains('*') || s.contains('?'))
        .unwrap_or(false)
}

/// Resolves an `Include` pattern (possibly a glob) relative to `ssh_dir`.
/// Best effort: unreadable directories simply yield no matches.
fn resolve_glob(ssh_dir: &Path, pattern: &str) -> Vec<PathBuf> {
    let expanded = expand_tilde(pattern);
    let full = if expanded.is_absolute() {
        expanded
    } else {
        ssh_dir.join(expanded)
    };
    if !has_wildcard(&full) {
        return if full.is_file() { vec![full] } else { vec![] };
    }
    let parent = full
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| ssh_dir.to_path_buf());
    let file_pattern = full
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let mut matches = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&parent) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if glob_match(&file_pattern, name) && entry.path().is_file() {
                    matches.push(entry.path());
                }
            }
        }
    }
    matches.sort();
    matches
}

fn flatten(path: &Path, ssh_dir: &Path, depth: u32, out: &mut Vec<String>) -> io::Result<()> {
    if depth > MAX_INCLUDE_DEPTH {
        return Ok(());
    }
    let text = std::fs::read_to_string(path)?;
    for raw_line in text.lines() {
        if let Some((key, value)) = split_key_value(raw_line) {
            if key.eq_ignore_ascii_case("include") {
                for pattern in split_words(&value) {
                    for included in resolve_glob(ssh_dir, &pattern) {
                        // Best effort: a broken included file is skipped.
                        let _ = flatten(&included, ssh_dir, depth + 1, out);
                    }
                }
                continue;
            }
        }
        out.push(raw_line.to_string());
    }
    Ok(())
}

/// Parses a real `~/.ssh/config` file, following `Include` directives
/// (resolved relative to the file's own directory, matching OpenSSH's
/// `~/.ssh`-relative behaviour when given the real config path).
pub fn parse_config(path: &Path) -> io::Result<Vec<ParsedHost>> {
    let ssh_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut flat = Vec::new();
    flatten(path, &ssh_dir, 0, &mut flat)?;
    Ok(parse_flat_lines(&flat))
}

/// A plan to import a set of parsed hosts. Building the plan does no I/O;
/// [`apply`] does the actual vault writes and identity file reads.
#[derive(Debug, Clone, Default)]
pub struct ImportPlan {
    pub hosts: Vec<ParsedHost>,
}

pub fn plan_ssh_config_import(parsed: Vec<ParsedHost>) -> ImportPlan {
    ImportPlan { hosts: parsed }
}

/// Imports a plan into `vault`. Hosts whose name already exists are skipped
/// (and reported); identity files are read from disk and deduped by
/// content, and an [`Identity`] is created linking each host's user to its
/// key. Encrypted identity files whose passphrase (if any was given in
/// `options`) doesn't work are stored as-is with no passphrase, to be
/// supplied later.
pub fn apply(
    vault: &mut Vault,
    plan: &ImportPlan,
    options: &ImportOptions,
) -> Result<ImportReport> {
    let mut report = ImportReport::default();
    // Dedupe identical key files by their raw content.
    let mut key_cache: HashMap<String, String> = HashMap::new();

    for host in &plan.hosts {
        if vault.find::<Host>(&host.alias).is_ok() {
            report.hosts_skipped.push(host.alias.clone());
            continue;
        }

        let mut identity_id: Option<String> = None;
        for path in &host.identity_files {
            let text = match std::fs::read_to_string(path) {
                Ok(t) => t,
                Err(_) => continue, // best effort: file missing/unreadable
            };

            let key_id = if let Some(existing) = key_cache.get(&text) {
                existing.clone()
            } else {
                let passphrase = options.key_passphrase.as_deref();
                let item = match crate::keygen::import(&text, passphrase) {
                    Ok(imported) => imported.to_vault_item(
                        &format!("{}-key", host.alias),
                        passphrase.filter(|_| imported.was_encrypted),
                    ),
                    Err(crate::keygen::KeygenError::PassphraseRequired) => {
                        report.warnings.push(format!(
                            "{}: identity file {} is encrypted; no passphrase supplied, stored as-is",
                            host.alias, path
                        ));
                        SshKey {
                            name: format!("{}-key", host.alias),
                            private_key: text.clone(),
                            public_key: crate::keygen::peek_public_key(&text).unwrap_or_default(),
                            passphrase: None,
                        }
                    }
                    Err(crate::keygen::KeygenError::WrongPassphrase) => {
                        report.warnings.push(format!(
                            "{}: identity file {} is encrypted; supplied passphrase doesn't match, stored as-is",
                            host.alias, path
                        ));
                        SshKey {
                            name: format!("{}-key", host.alias),
                            private_key: text.clone(),
                            public_key: crate::keygen::peek_public_key(&text).unwrap_or_default(),
                            passphrase: None,
                        }
                    }
                    Err(e) => {
                        report.warnings.push(format!(
                            "{}: could not import identity file {}: {}",
                            host.alias, path, e
                        ));
                        continue;
                    }
                };
                let id = vault.put(None, &item)?;
                report.keys_added += 1;
                key_cache.insert(text, id.clone());
                id
            };

            let identity = Identity {
                name: format!("{}-identity", host.alias),
                username: host.user.clone(),
                password: None,
                key_id: Some(key_id),
            };
            let id = vault.put(None, &identity)?;
            report.identities_added += 1;
            if identity_id.is_none() {
                identity_id = Some(id);
            }
        }

        let mut h = Host {
            name: host.alias.clone(),
            hostname: host.hostname.clone().unwrap_or_else(|| host.alias.clone()),
            port: host.port.unwrap_or(0),
            username: host.user.clone(),
            identity_id,
            ..Default::default()
        };
        if let Some(pj) = &host.proxy_jump {
            let first_hop = pj.split(',').next().unwrap_or(pj).trim().to_string();
            h.jump_host = Some(first_hop);
            if pj.contains(',') {
                h.notes = Some(format!("ProxyJump: {pj}"));
            }
        }
        vault.put(None, &h)?;
        report.hosts_added += 1;
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::kdf::KdfParams;
    use crate::vault::Created;

    fn new_vault(dir: &Path) -> Created {
        Vault::create_with_kdf(
            &dir.join("v.db"),
            "pw",
            true,
            KdfParams::insecure_for_tests(),
        )
        .unwrap()
    }

    #[test]
    fn simple_host_block() {
        let hosts = parse_text(
            r#"
Host web1
    HostName 10.0.0.1
    User root
    Port 2222
"#,
        );
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "web1");
        assert_eq!(hosts[0].hostname, Some("10.0.0.1".into()));
        assert_eq!(hosts[0].user, Some("root".into()));
        assert_eq!(hosts[0].port, Some(2222));
    }

    #[test]
    fn equals_syntax_and_quoting() {
        let hosts = parse_text(
            r#"
Host web1
    HostName=10.0.0.1
    User = "root"
    IdentityFile = "~/.ssh/id with space"
"#,
        );
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].hostname, Some("10.0.0.1".into()));
        assert_eq!(hosts[0].user, Some("root".into()));
        assert_eq!(hosts[0].identity_files.len(), 1);
        assert!(hosts[0].identity_files[0].ends_with("id with space"));
    }

    #[test]
    fn multiple_aliases_per_host_line() {
        let hosts = parse_text(
            r#"
Host web1 web2
    HostName 10.0.0.1
"#,
        );
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].alias, "web1");
        assert_eq!(hosts[1].alias, "web2");
        assert_eq!(hosts[0].hostname, Some("10.0.0.1".into()));
        assert_eq!(hosts[1].hostname, Some("10.0.0.1".into()));
    }

    #[test]
    fn wildcard_block_applies_defaults_to_later_hosts_only() {
        let hosts = parse_text(
            r#"
Host early
    HostName 1.2.3.4

Host *
    User root
    Port 2200

Host later
    HostName 5.6.7.8
"#,
        );
        assert_eq!(hosts.len(), 2);
        let early = hosts.iter().find(|h| h.alias == "early").unwrap();
        let later = hosts.iter().find(|h| h.alias == "later").unwrap();
        // Declared before the wildcard block: unaffected.
        assert_eq!(early.user, None);
        assert_eq!(early.port, None);
        // Declared after: picks up the wildcard defaults.
        assert_eq!(later.user, Some("root".into()));
        assert_eq!(later.port, Some(2200));
        assert_eq!(later.hostname, Some("5.6.7.8".into()));
    }

    #[test]
    fn host_own_setting_wins_over_default() {
        let hosts = parse_text(
            r#"
Host *
    Port 2200

Host web1
    HostName 1.2.3.4
    Port 22
"#,
        );
        assert_eq!(hosts[0].port, Some(22));
    }

    #[test]
    fn proxy_jump_single_hop() {
        let hosts = parse_text(
            r#"
Host web1
    HostName 1.2.3.4
    ProxyJump bastion
"#,
        );
        assert_eq!(hosts[0].proxy_jump, Some("bastion".into()));
    }

    #[test]
    fn proxy_jump_comma_list_kept_in_full() {
        let hosts = parse_text(
            r#"
Host web1
    HostName 1.2.3.4
    ProxyJump bastion1,bastion2
"#,
        );
        assert_eq!(hosts[0].proxy_jump, Some("bastion1,bastion2".into()));
    }

    #[test]
    fn comments_and_blank_lines_ignored() {
        let hosts =
            parse_text("# a comment\n\nHost web1\n  # nested comment\n  HostName 1.2.3.4\n\n");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].hostname, Some("1.2.3.4".into()));
    }

    #[test]
    fn include_directive_follows_other_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config"),
            "Include extra.conf\n\nHost web1\n    HostName 1.2.3.4\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("extra.conf"),
            "Host included\n    HostName 9.9.9.9\n",
        )
        .unwrap();

        let hosts = parse_config(&dir.path().join("config")).unwrap();
        assert_eq!(hosts.len(), 2);
        assert!(hosts.iter().any(|h| h.alias == "included"));
        assert!(hosts.iter().any(|h| h.alias == "web1"));
    }

    #[test]
    fn apply_imports_identity_file_and_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let generated =
            crate::keygen::generate(crate::keygen::KeyAlgorithm::Ed25519, "t", None).unwrap();
        let key_path = dir.path().join("id_ed25519");
        std::fs::write(&key_path, generated.private_key_openssh.as_str()).unwrap();

        let hosts = vec![
            ParsedHost {
                alias: "web1".into(),
                hostname: Some("1.2.3.4".into()),
                user: Some("root".into()),
                identity_files: vec![key_path.to_string_lossy().into_owned()],
                ..Default::default()
            },
            ParsedHost {
                alias: "web2".into(),
                hostname: Some("5.6.7.8".into()),
                user: Some("root".into()),
                identity_files: vec![key_path.to_string_lossy().into_owned()],
                ..Default::default()
            },
        ];
        let plan = plan_ssh_config_import(hosts);

        let Created { mut vault, .. } = new_vault(dir.path());
        let report = apply(&mut vault, &plan, &ImportOptions::default()).unwrap();

        assert_eq!(report.hosts_added, 2);
        assert_eq!(report.keys_added, 1, "identical key files must be deduped");
        assert_eq!(report.identities_added, 2);
        assert_eq!(vault.list::<SshKey>().unwrap().len(), 1);
        assert_eq!(vault.list::<Identity>().unwrap().len(), 2);
        assert_eq!(vault.list::<Host>().unwrap().len(), 2);
    }

    #[test]
    fn apply_skips_existing_host() {
        let dir = tempfile::tempdir().unwrap();
        let Created { mut vault, .. } = new_vault(dir.path());
        vault
            .put(
                None,
                &Host {
                    name: "web1".into(),
                    hostname: "already-here".into(),
                    ..Default::default()
                },
            )
            .unwrap();

        let plan = plan_ssh_config_import(vec![
            ParsedHost {
                alias: "web1".into(),
                hostname: Some("1.2.3.4".into()),
                ..Default::default()
            },
            ParsedHost {
                alias: "web2".into(),
                hostname: Some("5.6.7.8".into()),
                ..Default::default()
            },
        ]);

        let report = apply(&mut vault, &plan, &ImportOptions::default()).unwrap();
        assert_eq!(report.hosts_added, 1);
        assert_eq!(report.hosts_skipped, vec!["web1".to_string()]);
        assert_eq!(vault.list::<Host>().unwrap().len(), 2);
        assert_eq!(
            vault.find::<Host>("web1").unwrap().1.hostname,
            "already-here"
        );
    }

    #[test]
    fn apply_stores_encrypted_identity_with_unknown_passphrase_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let generated =
            crate::keygen::generate(crate::keygen::KeyAlgorithm::Ed25519, "t", Some("hunter2"))
                .unwrap();
        let key_path = dir.path().join("id_ed25519");
        std::fs::write(&key_path, generated.private_key_openssh.as_str()).unwrap();

        let plan = plan_ssh_config_import(vec![ParsedHost {
            alias: "web1".into(),
            hostname: Some("1.2.3.4".into()),
            identity_files: vec![key_path.to_string_lossy().into_owned()],
            ..Default::default()
        }]);

        let Created { mut vault, .. } = new_vault(dir.path());
        let report = apply(&mut vault, &plan, &ImportOptions::default()).unwrap();

        assert_eq!(report.keys_added, 1);
        assert!(!report.warnings.is_empty());
        let (_, key) = &vault.list::<SshKey>().unwrap()[0];
        assert!(key
            .private_key
            .contains("-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert!(key.passphrase.is_none());
        // The public key material can still be recovered without the
        // passphrase (the comment, however, lives in the encrypted section).
        assert!(key
            .public_key
            .split_whitespace()
            .take(2)
            .eq(generated.public_key_openssh.split_whitespace().take(2)));
    }
}
