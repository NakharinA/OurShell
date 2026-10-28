//! `item.*` handlers: typed vault CRUD addressed by kind string.

use std::sync::Arc;

use ourshell_core::import::{self, ImportOptions, ImportReport};
use ourshell_core::keygen::{self, KeyAlgorithm};
use ourshell_core::vault::{
    Group, Host, Identity, Item, KnownHost, Snippet, SshKey, Vault, VaultError,
};
use ourshell_proto::agent::{
    method, ImportFileParams, ItemListParams, ItemPutParams, ItemRecord, ItemRef,
    KeyGenerateParams, KeyImportParams,
};
use ourshell_proto::rpc;
use serde_json::{json, Value};

use super::{err, parse, to_value, RpcResult, State};

/// Calls `$f::<T>` with the item type named by `$kind`.
macro_rules! with_kind {
    ($kind:expr, $f:ident ( $($arg:expr),* )) => {
        match $kind {
            "host" => $f::<Host>($($arg),*),
            "identity" => $f::<Identity>($($arg),*),
            "key" => $f::<SshKey>($($arg),*),
            "snippet" => $f::<Snippet>($($arg),*),
            "group" => $f::<Group>($($arg),*),
            "known_host" => $f::<KnownHost>($($arg),*),
            other => Err(err(rpc::INVALID_PARAMS, format!("unknown item kind `{other}`"))),
        }
    };
}

fn record<T: Item>(id: String, item: &T) -> ItemRecord {
    ItemRecord {
        id,
        kind: T::KIND.into(),
        item: serde_json::to_value(item).unwrap_or(Value::Null),
    }
}

/// Removes secrets before items leave the agent.
pub fn redact(mut rec: ItemRecord) -> ItemRecord {
    if let Some(obj) = rec.item.as_object_mut() {
        for k in ["password", "private_key", "passphrase"] {
            if let Some(v) = obj.get_mut(k) {
                if !v.is_null() {
                    *v = json!("********");
                }
            }
        }
    }
    rec
}

fn list<T: Item>(v: &mut Vault) -> Result<Vec<ItemRecord>, VaultError> {
    Ok(v.list::<T>()?
        .into_iter()
        .map(|(id, it)| redact(record(id, &it)))
        .collect())
}

fn get<T: Item>(v: &mut Vault, id: &str) -> Result<ItemRecord, VaultError> {
    let (id, it) = v.find::<T>(id)?;
    Ok(redact(record(id, &it)))
}

fn put<T: Item>(v: &mut Vault, p: &ItemPutParams) -> Result<String, VaultError> {
    let mut incoming = p.item.clone();
    // Keep secrets that came back redacted (clients never see the real value).
    if let (Some(id), Some(obj)) = (p.id.as_deref(), incoming.as_object_mut()) {
        if let Ok(existing) = v.get::<T>(id) {
            let existing = serde_json::to_value(&existing)?;
            for k in ["password", "private_key", "passphrase"] {
                if obj.get(k).and_then(Value::as_str) == Some("********") {
                    obj.insert(k.into(), existing.get(k).cloned().unwrap_or(Value::Null));
                }
            }
        }
    }
    let item: T = serde_json::from_value(incoming)?;
    if let Some(name) = serde_json::to_value(&item)?
        .get("name")
        .and_then(Value::as_str)
    {
        if let Ok((other, _)) = v.find::<T>(name) {
            if Some(other.as_str()) != p.id.as_deref() {
                return Err(VaultError::Exists(format!("{} `{name}`", T::KIND).into()));
            }
        }
    }
    v.put(p.id.as_deref(), &item)
}

fn delete<T: Item>(v: &mut Vault, id: &str) -> Result<(), VaultError> {
    let (id, _) = v.find::<T>(id)?;
    v.delete(&id)
}

fn wrap<T>(r: Result<T, VaultError>) -> Result<T, rpc::RpcError> {
    r.map_err(|e| match e {
        VaultError::Exists(what) => err(
            rpc::INVALID_PARAMS,
            format!("{} already exists", what.display()),
        ),
        other => super::vault_err(other),
    })
}

fn list_kind<T: Item>(v: &mut Vault) -> RpcResult {
    to_value(wrap(list::<T>(v))?)
}
fn get_kind<T: Item>(v: &mut Vault, id: &str) -> RpcResult {
    to_value(wrap(get::<T>(v, id))?)
}
/// Returns the stored (redacted) record; its `id` field is what callers need.
fn put_kind<T: Item>(v: &mut Vault, p: &ItemPutParams) -> RpcResult {
    let id = wrap(put::<T>(v, p))?;
    to_value(wrap(get::<T>(v, &id))?)
}
fn delete_kind<T: Item>(v: &mut Vault, id: &str) -> RpcResult {
    wrap(delete::<T>(v, id))?;
    Ok(json!({}))
}

pub async fn dispatch(state: &Arc<State>, m: &str, params: Value) -> RpcResult {
    match m {
        method::ITEM_LIST => {
            let p: ItemListParams = parse(params)?;
            state
                .with_vault(|v| Ok(with_kind!(p.kind.as_str(), list_kind(v))))
                .await?
        }
        method::ITEM_GET => {
            let p: ItemRef = parse(params)?;
            state
                .with_vault(|v| Ok(with_kind!(p.kind.as_str(), get_kind(v, &p.id))))
                .await?
        }
        method::ITEM_PUT => {
            let p: ItemPutParams = parse(params)?;
            state
                .with_vault(|v| Ok(with_kind!(p.kind.as_str(), put_kind(v, &p))))
                .await?
        }
        method::ITEM_DELETE => {
            let p: ItemRef = parse(params)?;
            state
                .with_vault(|v| Ok(with_kind!(p.kind.as_str(), delete_kind(v, &p.id))))
                .await?
        }
        method::KEY_GENERATE => key_generate(state, parse(params)?).await,
        method::KEY_IMPORT => key_import(state, parse(params)?).await,
        method::IMPORT_SSH_CONFIG => {
            let p: ImportFileParams = parse(params)?;
            let parsed = import::ssh_config::parse_text(&p.contents);
            let plan = import::ssh_config::plan_ssh_config_import(parsed);
            let report = state
                .with_vault(|v| {
                    import::ssh_config::apply(v, &plan, &ImportOptions::default())
                        .map_err(import_err)
                })
                .await?;
            Ok(report_json(&report))
        }
        method::IMPORT_TERMIUS_CSV => {
            let p: ImportFileParams = parse(params)?;
            let parsed = import::termius_csv::parse_termius_csv(&p.contents)
                .map_err(|e| err(rpc::INVALID_PARAMS, e.to_string()))?;
            let plan = import::termius_csv::plan_termius_csv_import(parsed);
            let report = state
                .with_vault(|v| {
                    import::termius_csv::apply(v, &plan, &ImportOptions::default())
                        .map_err(import_err)
                })
                .await?;
            Ok(report_json(&report))
        }
        _ => Err(err(rpc::METHOD_NOT_FOUND, format!("unknown method `{m}`"))),
    }
}

fn import_err(e: import::ImportError) -> VaultError {
    match e {
        import::ImportError::Vault(v) => v,
        other => VaultError::NotFound(format!("import failed: {other}")),
    }
}

fn report_json(r: &ImportReport) -> Value {
    json!({
        "hosts_added": r.hosts_added,
        "hosts_skipped": r.hosts_skipped,
        "keys_added": r.keys_added,
        "identities_added": r.identities_added,
        "groups_added": r.groups_added,
        "warnings": r.warnings,
    })
}

async fn key_generate(state: &Arc<State>, p: KeyGenerateParams) -> RpcResult {
    let alg = match p.algorithm.to_ascii_lowercase().as_str() {
        "ed25519" => KeyAlgorithm::Ed25519,
        "rsa" => KeyAlgorithm::Rsa {
            bits: p.bits.unwrap_or(3072),
        },
        other => {
            return Err(err(
                rpc::INVALID_PARAMS,
                format!("unknown algorithm `{other}` (use ed25519 or rsa)"),
            ))
        }
    };
    let name = p.name.clone();
    let pass = p.passphrase.clone();
    let key = tokio::task::spawn_blocking(move || keygen::generate(alg, &name, pass.as_deref()))
        .await
        .map_err(|e| err(rpc::INTERNAL_ERROR, e.to_string()))?
        .map_err(|e| err(rpc::INVALID_PARAMS, e.to_string()))?;
    let item = key.to_vault_item(&p.name, p.passphrase.as_deref());
    let id = state
        .with_vault(|v| {
            if v.find::<SshKey>(&p.name).is_ok() {
                return Err(VaultError::Exists(format!("key `{}`", p.name).into()));
            }
            v.put(None, &item)
        })
        .await?;
    Ok(
        json!({"id": id, "public_key": key.public_key_openssh, "fingerprint": key.fingerprint_sha256}),
    )
}

async fn key_import(state: &Arc<State>, p: KeyImportParams) -> RpcResult {
    let key = keygen::import(&p.private_key, p.passphrase.as_deref())
        .map_err(|e| err(rpc::INVALID_PARAMS, e.to_string()))?;
    let item = key.to_vault_item(&p.name, p.passphrase.as_deref());
    let id = state
        .with_vault(|v| {
            if v.find::<SshKey>(&p.name).is_ok() {
                return Err(VaultError::Exists(format!("key `{}`", p.name).into()));
            }
            v.put(None, &item)
        })
        .await?;
    Ok(
        json!({"id": id, "public_key": key.public_key_openssh, "fingerprint": key.fingerprint_sha256, "algorithm": key.algorithm}),
    )
}
