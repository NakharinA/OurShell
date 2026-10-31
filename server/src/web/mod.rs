//! The server's built-in web pages: the first-run setup wizard (`/setup`)
//! and the admin page (`/admin`). Plain HTML/CSS/JS embedded in the binary,
//! so the image needs no separate web server or build step.

pub mod admin;
pub mod password;
pub mod setup;

use std::sync::Arc;
use std::time::Duration;

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tokio::sync::watch;

const APP_CSS: &str = include_str!("../../web/app.css");
const COMMON_JS: &str = include_str!("../../web/common.js");
const SETUP_JS: &str = include_str!("../../web/setup.js");
const ADMIN_JS: &str = include_str!("../../web/admin.js");
const LOGO_SVG: &str = include_str!("../../../assets/logo/ourshell.svg");
pub(crate) const SETUP_HTML: &str = include_str!("../../web/setup.html");
pub(crate) const ADMIN_HTML: &str = include_str!("../../web/admin.html");

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
                   connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Asks the serve loop in [`crate::run`] to rebuild the server from the
/// current configuration (after the setup wizard or an admin settings
/// change), keeping the listening socket open.
#[derive(Clone)]
pub struct Reload(Arc<watch::Sender<u64>>);

impl Default for Reload {
    fn default() -> Self {
        Self(Arc::new(watch::channel(0).0))
    }
}

impl Reload {
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.subscribe()
    }

    /// Reloads shortly after the current response has been sent.
    pub fn trigger_soon(&self) {
        let tx = self.0.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            tx.send_modify(|n| *n += 1);
        });
    }
}

/// An HTML page with a strict CSP; scripts and styles come only from the
/// `/web/*` routes below.
pub(crate) fn page(html: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CONTENT_SECURITY_POLICY, CSP),
            (header::X_FRAME_OPTIONS, "DENY"),
            (header::REFERRER_POLICY, "no-referrer"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        html,
    )
        .into_response()
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        body,
    )
        .into_response()
}

/// `/web/*` static assets, shared by both pages.
pub(crate) fn assets<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route(
            "/web/app.css",
            get(|| async { asset("text/css; charset=utf-8", APP_CSS) }),
        )
        .route(
            "/web/common.js",
            get(|| async { asset("text/javascript; charset=utf-8", COMMON_JS) }),
        )
        .route(
            "/web/setup.js",
            get(|| async { asset("text/javascript; charset=utf-8", SETUP_JS) }),
        )
        .route(
            "/web/admin.js",
            get(|| async { asset("text/javascript; charset=utf-8", ADMIN_JS) }),
        )
        .route(
            "/web/logo.svg",
            get(|| async { asset("image/svg+xml", LOGO_SVG) }),
        )
}

/// `postgres://user:pass@host:5432/db?x` -> `host:5432/db`, for display.
pub(crate) fn redact_database_url(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
    rest.split('?').next().unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_credentials() {
        assert_eq!(
            redact_database_url("postgres://u:p%40ss@db.local:5432/ourshell?sslmode=require"),
            "db.local:5432/ourshell"
        );
        assert_eq!(redact_database_url("postgres://db/x"), "db/x");
    }
}
