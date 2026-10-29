//! Tauri commands exposed to the webview. These are intentionally thin: the only
//! thing the desktop app's Rust side knows how to do is forward a JSON-RPC call to
//! the agent and hand back whatever it said. All protocol knowledge (methods,
//! params, errors) lives in the TypeScript RPC client (`src/lib/rpc.ts`), which
//! mirrors `crates/proto/src/agent.rs` and `crates/proto/src/rpc.rs`.

use ourshell_proto::rpc::RpcError;
use serde_json::Value;
use tauri::State;

use crate::agent::AgentHandle;

/// The bridge's current connection status (`{connected, detail}`).
#[tauri::command]
pub fn agent_status() -> Value {
    crate::agent::last_status()
}

/// Calls one JSON-RPC method on the agent and returns its `result` (or its
/// `error`, which the frontend matches on `.code` per `docs/agent-protocol.md`).
#[tauri::command]
pub async fn rpc_call(
    agent: State<'_, AgentHandle>,
    method: String,
    params: Value,
) -> Result<Value, RpcError> {
    agent.inner().clone().call(method, params).await
}

/// Result of [`check_latest_release`], mirrored on the TypeScript side by
/// `FallbackUpdateCheck` in `src/lib/stores/update.ts`.
#[derive(serde::Serialize)]
pub struct FallbackUpdateCheck {
    pub current: String,
    pub latest: Option<String>,
    pub available: bool,
    pub html_url: Option<String>,
    pub notes: Option<String>,
}

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
}

/// Fallback update check for builds without the `updater` cargo feature
/// (which have no `tauri-plugin-updater`, so `@tauri-apps/plugin-updater`'s
/// `check()` fails in the webview -- see `src/lib/stores/update.ts`).
///
/// Queries `https://api.github.com/repos/NakharinA/OurShell/releases/latest`
/// -- the only place OurShell ever looks for updates -- from Rust rather
/// than the webview, specifically so the app's CSP `connect-src` never has
/// to allow `api.github.com` (see `tauri.conf.json`). A 404 (no releases
/// published yet) is reported as "no update available", not an error.
#[tauri::command]
pub async fn check_latest_release() -> Result<FallbackUpdateCheck, String> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get("https://api.github.com/repos/NakharinA/OurShell/releases/latest")
        .header("User-Agent", format!("ourshell-desktop/{current}"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("could not reach GitHub: {e}"))?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(FallbackUpdateCheck {
            current,
            latest: None,
            available: false,
            html_url: None,
            notes: None,
        });
    }
    if !resp.status().is_success() {
        return Err(format!("GitHub API returned {}", resp.status()));
    }

    let release: GithubRelease = resp
        .json()
        .await
        .map_err(|e| format!("could not parse GitHub API response: {e}"))?;
    let latest_version = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);
    let available = match (
        semver::Version::parse(&current),
        semver::Version::parse(latest_version),
    ) {
        (Ok(cur), Ok(lat)) => lat > cur,
        _ => false,
    };

    Ok(FallbackUpdateCheck {
        current,
        latest: Some(release.tag_name),
        available,
        html_url: Some(release.html_url),
        notes: release.body,
    })
}
