//! `item.*` handlers: typed vault CRUD addressed by kind string.

use std::sync::Arc;

use ourshell_core::vault::{
    Group, Host, Identity, Item, KnownHost, Snippet, SshKey, Vault, VaultError,
};
use ourshell_proto::agent::{method, ItemListParams, ItemPutParams, ItemRecord, ItemRef};
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
fn put_kind<T: Item>(v: &mut Vault, p: &ItemPutParams) -> RpcResult {
    Ok(json!({ "id": wrap(put::<T>(v, p))? }))
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
        _ => Err(err(rpc::METHOD_NOT_FOUND, format!("unknown method `{m}`"))),
    }
}
