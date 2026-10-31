//! The `.osbk` backup file.
//!
//! ```text
//! header (plaintext, 59 bytes, also the AEAD's associated data)
//!   "OSBK1"                 magic + format version
//!   u8   kdf                1 = Argon2id v0x13
//!   u32  m_cost (KiB)  LE
//!   u32  t_cost        LE
//!   u32  p_cost        LE
//!   u8   salt length        16
//!   [16] salt
//!   [24] XChaCha20-Poly1305 nonce
//! ciphertext + 16-byte tag  = encrypt(gzip(payload))
//! ```
//!
//! The key is Argon2id(passphrase, salt) with the parameters in the header,
//! so changing the default cost later never breaks old files. The payload
//! is a flat list of named entries (`u32 name length | name | u64 data
//! length | data`): `manifest.json`, `config.toml`, then one
//! `tables/<name>.csv` per table (Postgres `COPY ... CSV HEADER`), in the
//! manifest's foreign-key-safe load order.

use std::io::{Read, Write};

use anyhow::{anyhow, bail, ensure, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const MAGIC: &[u8; 5] = b"OSBK1";
pub const PAYLOAD_FORMAT: u32 = 1;
const KDF_ARGON2ID: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = MAGIC.len() + 1 + 12 + 1 + SALT_LEN + NONCE_LEN;
/// Refuse absurd costs from a corrupted or hostile header (1 GiB, 16 passes).
const MAX_M_COST_KIB: u32 = 1024 * 1024;
const MAX_T_COST: u32 = 16;
const MAX_P_COST: u32 = 16;

pub const MANIFEST_ENTRY: &str = "manifest.json";
pub const CONFIG_ENTRY: &str = "config.toml";
pub const TABLE_PREFIX: &str = "tables/";

/// Tables whose rows are pure churn: short-lived protocol state, sign-in
/// sessions and the audit log (every sign-in adds a session and a `login`
/// event). They are still backed up, but don't count as a change on their
/// own, so a server whose only activity is people signing in doesn't
/// produce a new backup every day.
pub const VOLATILE_TABLES: &[&str] = &[
    "sessions",
    "events",
    "pending_registrations",
    "pending_logins",
    "pending_password_changes",
    "oidc_requests",
];

/// Argon2id cost for new backup files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl Default for KdfParams {
    /// 64 MiB, 3 passes: a backup is encrypted at most a few times a day.
    fn default() -> Self {
        Self {
            m_cost_kib: 64 * 1024,
            t_cost: 3,
            p_cost: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    pub rows: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// [`PAYLOAD_FORMAT`].
    pub format: u32,
    pub server_version: String,
    pub created_at: DateTime<Utc>,
    /// Versions from `_sqlx_migrations` (successful ones), ascending.
    pub migrations: Vec<i64>,
    /// Every table in the file, in foreign-key-safe load order.
    pub tables: Vec<TableInfo>,
    /// [`Bundle::content_hash`], hex.
    pub content_hash: String,
}

/// A decrypted backup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    pub manifest: Manifest,
    pub config_toml: String,
    /// `(table, CSV with a header row)` in load order.
    pub tables: Vec<(String, Vec<u8>)>,
}

impl Bundle {
    /// SHA-256 over what makes two backups different: the schema version,
    /// `config.toml` and every non-[volatile](VOLATILE_TABLES) table. Leaves
    /// out `created_at` and the server version, so an unchanged server
    /// hashes the same on every run.
    pub fn content_hash(&self) -> [u8; 32] {
        content_hash(&self.manifest.migrations, &self.config_toml, &self.tables)
    }

    pub fn table(&self, name: &str) -> Option<&[u8]> {
        self.tables
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, d)| d.as_slice())
    }

    /// The unencrypted payload (before compression).
    pub fn to_payload(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        put_entry(
            &mut out,
            MANIFEST_ENTRY,
            &serde_json::to_vec_pretty(&self.manifest)?,
        );
        put_entry(&mut out, CONFIG_ENTRY, self.config_toml.as_bytes());
        for (name, data) in &self.tables {
            put_entry(&mut out, &format!("{TABLE_PREFIX}{name}.csv"), data);
        }
        Ok(out)
    }

    pub fn from_payload(mut p: &[u8]) -> Result<Self> {
        let mut manifest = None;
        let mut config_toml = None;
        let mut tables = Vec::new();
        while !p.is_empty() {
            let (name, data, rest) = take_entry(p)?;
            p = rest;
            if name == MANIFEST_ENTRY {
                manifest = Some(
                    serde_json::from_slice::<Manifest>(data).context("the manifest is invalid")?,
                );
            } else if name == CONFIG_ENTRY {
                config_toml =
                    Some(String::from_utf8(data.to_vec()).context("config.toml isn't UTF-8")?);
            } else if let Some(t) = name
                .strip_prefix(TABLE_PREFIX)
                .and_then(|n| n.strip_suffix(".csv"))
            {
                tables.push((t.to_string(), data.to_vec()));
            }
            // Unknown entries are skipped (room for later additions).
        }
        let manifest = manifest.ok_or_else(|| anyhow!("the backup has no manifest"))?;
        ensure!(
            manifest.format == PAYLOAD_FORMAT,
            "unsupported backup payload format {} (this server reads {PAYLOAD_FORMAT})",
            manifest.format
        );
        let bundle = Self {
            manifest,
            config_toml: config_toml.ok_or_else(|| anyhow!("the backup has no config.toml"))?,
            tables,
        };
        bundle.verify()?;
        Ok(bundle)
    }

    /// Checks the manifest against the contents: same tables, and the
    /// recorded content hash matches.
    pub fn verify(&self) -> Result<()> {
        let listed: Vec<&str> = self
            .manifest
            .tables
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        let present: Vec<&str> = self.tables.iter().map(|(n, _)| n.as_str()).collect();
        ensure!(
            listed == present,
            "the manifest lists tables {listed:?} but the backup holds {present:?}"
        );
        ensure!(
            hex(&self.content_hash()) == self.manifest.content_hash,
            "the backup's contents don't match its manifest"
        );
        Ok(())
    }
}

pub fn content_hash(
    migrations: &[i64],
    config_toml: &str,
    tables: &[(String, Vec<u8>)],
) -> [u8; 32] {
    let mut h = Sha256::new();
    let mut part = |label: &[u8], data: &[u8]| {
        h.update((label.len() as u64).to_le_bytes());
        h.update(label);
        h.update((data.len() as u64).to_le_bytes());
        h.update(data);
    };
    part(b"ourshell-backup-hash", b"v1");
    let versions: Vec<u8> = migrations.iter().flat_map(|v| v.to_le_bytes()).collect();
    part(b"migrations", &versions);
    part(b"config.toml", config_toml.as_bytes());
    for (name, data) in tables {
        if !VOLATILE_TABLES.contains(&name.as_str()) {
            part(name.as_bytes(), data);
        }
    }
    h.finalize().into()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn put_entry(out: &mut Vec<u8>, name: &str, data: &[u8]) {
    out.extend_from_slice(&(name.len() as u32).to_le_bytes());
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    out.extend_from_slice(data);
}

fn take_entry(p: &[u8]) -> Result<(&str, &[u8], &[u8])> {
    let bad = || anyhow!("the backup payload is truncated or corrupt");
    let (len, p) = p.split_at_checked(4).ok_or_else(bad)?;
    let len = u32::from_le_bytes(len.try_into()?) as usize;
    let (name, p) = p.split_at_checked(len).ok_or_else(bad)?;
    let name = std::str::from_utf8(name).map_err(|_| bad())?;
    let (len, p) = p.split_at_checked(8).ok_or_else(bad)?;
    let len = usize::try_from(u64::from_le_bytes(len.try_into()?)).map_err(|_| bad())?;
    let (data, rest) = p.split_at_checked(len).ok_or_else(bad)?;
    Ok((name, data, rest))
}

fn derive_key(passphrase: &str, salt: &[u8], k: KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(k.m_cost_kib, k.t_cost, k.p_cost, Some(32))
        .map_err(|e| anyhow!("argon2 params: {e}"))?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, key.as_mut())
        .map_err(|e| anyhow!("argon2: {e}"))?;
    Ok(key)
}

/// Compresses and encrypts a bundle into `.osbk` bytes.
pub fn seal(bundle: &Bundle, passphrase: &str, kdf: KdfParams) -> Result<Vec<u8>> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&bundle.to_payload()?)?;
    let compressed = gz.finish()?;

    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.push(KDF_ARGON2ID);
    header.extend_from_slice(&kdf.m_cost_kib.to_le_bytes());
    header.extend_from_slice(&kdf.t_cost.to_le_bytes());
    header.extend_from_slice(&kdf.p_cost.to_le_bytes());
    header.push(SALT_LEN as u8);
    header.extend_from_slice(&salt);
    header.extend_from_slice(&nonce);
    debug_assert_eq!(header.len(), HEADER_LEN);

    let key = derive_key(passphrase, &salt, kdf)?;
    let ct = XChaCha20Poly1305::new((&*key).into())
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &compressed,
                aad: &header,
            },
        )
        .map_err(|_| anyhow!("encryption failed"))?;
    let mut out = header;
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Decrypts, decompresses and verifies `.osbk` bytes.
pub fn open(file: &[u8], passphrase: &str) -> Result<Bundle> {
    ensure!(
        file.len() > HEADER_LEN + 16 && file.starts_with(MAGIC),
        "not an OurShell backup file (expected it to start with {:?})",
        std::str::from_utf8(MAGIC).unwrap_or_default()
    );
    let (header, ct) = file.split_at(HEADER_LEN);
    let mut p = &header[MAGIC.len()..];
    let mut take = |n: usize| {
        let (a, b) = p.split_at(n);
        p = b;
        a
    };
    if take(1)[0] != KDF_ARGON2ID {
        bail!("unsupported key derivation in the backup header");
    }
    let u32_at = |b: &[u8]| u32::from_le_bytes(b.try_into().expect("4 bytes"));
    let kdf = KdfParams {
        m_cost_kib: u32_at(take(4)),
        t_cost: u32_at(take(4)),
        p_cost: u32_at(take(4)),
    };
    ensure!(
        kdf.m_cost_kib <= MAX_M_COST_KIB && kdf.t_cost <= MAX_T_COST && kdf.p_cost <= MAX_P_COST,
        "the backup header asks for an unreasonable key derivation cost"
    );
    ensure!(
        take(1)[0] as usize == SALT_LEN,
        "unsupported salt length in the backup header"
    );
    let salt = take(SALT_LEN).to_vec();
    let nonce: [u8; NONCE_LEN] = take(NONCE_LEN).try_into().expect("nonce length");

    let key = derive_key(passphrase, &salt, kdf)?;
    let compressed = XChaCha20Poly1305::new((&*key).into())
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: ct,
                aad: header,
            },
        )
        .map_err(|_| {
            anyhow!("can't decrypt the backup: wrong passphrase, or the file is damaged")
        })?;
    let mut payload = Vec::new();
    flate2::read::GzDecoder::new(compressed.as_slice())
        .read_to_end(&mut payload)
        .context("the backup's compressed data is corrupt")?;
    Bundle::from_payload(&payload)
}

/// `ourshell-backup-20261101T020304Z.osbk`.
pub fn file_name(at: DateTime<Utc>) -> String {
    format!("ourshell-backup-{}.osbk", at.format("%Y%m%dT%H%M%SZ"))
}

/// Whether `name` (the part after the prefix) is one of our backup files.
pub fn is_backup_name(name: &str) -> bool {
    name.strip_prefix("ourshell-backup-")
        .and_then(|s| s.strip_suffix(".osbk"))
        .is_some_and(|stamp| {
            stamp.len() == 16
                && stamp
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b == b'T' || b == b'Z')
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Cheap enough for debug-mode tests.
    pub(crate) const FAST: KdfParams = KdfParams {
        m_cost_kib: 64,
        t_cost: 1,
        p_cost: 1,
    };

    pub(crate) fn sample(created_at: DateTime<Utc>, sessions: &str) -> Bundle {
        let tables = vec![
            ("orgs".to_string(), b"id,name\n1,Acme\n".to_vec()),
            (
                "sessions".to_string(),
                format!("id,user_id\n{sessions}\n").into_bytes(),
            ),
        ];
        let migrations = vec![1, 2];
        let config_toml = "server_master_key = \"k\"\n".to_string();
        let hash = hex(&content_hash(&migrations, &config_toml, &tables));
        Bundle {
            manifest: Manifest {
                format: PAYLOAD_FORMAT,
                server_version: "0.0.0-test".into(),
                created_at,
                migrations,
                tables: vec![
                    TableInfo {
                        name: "orgs".into(),
                        rows: 1,
                    },
                    TableInfo {
                        name: "sessions".into(),
                        rows: 1,
                    },
                ],
                content_hash: hash,
            },
            config_toml,
            tables,
        }
    }

    #[test]
    fn round_trip() {
        let b = sample(Utc::now(), "s1,u1");
        let file = seal(&b, "correct horse battery", FAST).unwrap();
        assert!(file.starts_with(MAGIC));
        assert!(!file.windows(4).any(|w| w == b"Acme"));
        assert_eq!(open(&file, "correct horse battery").unwrap(), b);
    }

    #[test]
    fn wrong_passphrase_fails_cleanly() {
        let file = seal(&sample(Utc::now(), "s"), "correct horse battery", FAST).unwrap();
        let err = open(&file, "wrong horse battery").unwrap_err().to_string();
        assert!(err.contains("wrong passphrase"), "{err}");
    }

    #[test]
    fn tampering_is_detected() {
        let file = seal(&sample(Utc::now(), "s"), "pw-pw-pw-pw-pw", FAST).unwrap();
        // A flipped ciphertext bit, and a header edit (it is the AAD).
        for at in [file.len() - 3, HEADER_LEN + 2, MAGIC.len() + 40] {
            let mut bad = file.clone();
            bad[at] ^= 1;
            assert!(open(&bad, "pw-pw-pw-pw-pw").is_err(), "byte {at}");
        }
        assert!(open(&file[..HEADER_LEN], "pw-pw-pw-pw-pw").is_err());
        assert!(open(b"PK\x03\x04 not a backup at all, really not", "x").is_err());
        // Contents that don't match the manifest are refused too.
        let mut b = sample(Utc::now(), "s");
        b.tables[0].1 = b"id,name\n1,Evil\n".to_vec();
        let file = seal(&b, "pw-pw-pw-pw-pw", FAST).unwrap();
        assert!(open(&file, "pw-pw-pw-pw-pw").is_err());
    }

    #[test]
    fn change_hash_ignores_timestamp_and_volatile_tables() {
        let a = sample(Utc::now(), "s1,u1");
        let later = sample(Utc::now() + chrono::Duration::days(3), "s2,u1");
        assert_ne!(a.manifest.created_at, later.manifest.created_at);
        assert_eq!(a.content_hash(), later.content_hash());

        let mut changed = a.clone();
        changed.tables[0].1 = b"id,name\n1,Acme Ltd\n".to_vec();
        assert_ne!(a.content_hash(), changed.content_hash());
        let mut changed = a.clone();
        changed.config_toml.push_str("public_url = \"https://x\"\n");
        assert_ne!(a.content_hash(), changed.content_hash());
        let mut changed = a.clone();
        changed.manifest.migrations.push(3);
        assert_ne!(a.content_hash(), changed.content_hash());
    }

    #[test]
    fn file_names() {
        let at = DateTime::parse_from_rfc3339("2026-11-01T02:03:04Z")
            .unwrap()
            .with_timezone(&Utc);
        let name = file_name(at);
        assert_eq!(name, "ourshell-backup-20261101T020304Z.osbk");
        assert!(is_backup_name(&name));
        assert!(!is_backup_name("ourshell-backup-latest.osbk"));
        assert!(!is_backup_name("other.osbk"));
    }
}
