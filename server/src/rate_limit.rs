//! Simple in-memory per-IP token bucket for the auth endpoints
//! (`register/*`, `login/*`). Good enough for a single self-hosted
//! instance; a multi-replica deployment behind a shared reverse proxy
//! should add rate limiting there too.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::state::AppState;

/// Applied only to the auth endpoints (`register/*`, `login/*`). When the
/// request has no `ConnectInfo` extension (e.g. an in-process test driving
/// the router directly without a bound socket, or `into_make_service`
/// without connect-info in a custom deployment), rate limiting is skipped
/// rather than guessed at. Reading it out of the request's extensions
/// (rather than as a separate middleware extractor argument) keeps this
/// function's signature to the single, unambiguous `State<S>` + `Request` +
/// `Next` arity axum's `from_fn` machinery resolves most reliably.
pub async fn rate_limit_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if let Some(blocked) = check(state.rate_limiter(), &request) {
        return blocked;
    }
    next.run(request).await
}

/// The same per-IP limit for routers that don't carry an [`AppState`]
/// (the setup wizard and the admin page), with their own limiter.
pub async fn limit_with(
    State(limiter): State<Arc<RateLimiter>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if let Some(blocked) = check(&limiter, &request) {
        return blocked;
    }
    next.run(request).await
}

fn check(limiter: &RateLimiter, request: &Request<Body>) -> Option<Response> {
    let ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())?;
    (!limiter.allow(ip)).then(|| {
        AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many requests, try again later",
        )
        .into_response()
    })
}

struct Bucket {
    window_start: Instant,
    count: u32,
}

pub struct RateLimiter {
    limit_per_min: u32,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

impl RateLimiter {
    pub fn new(limit_per_min: u32) -> Self {
        Self {
            limit_per_min,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Returns `true` if the request from `ip` is allowed under the current
    /// one-minute window, and records it.
    pub fn allow(&self, ip: IpAddr) -> bool {
        if self.limit_per_min == 0 {
            return true;
        }
        let mut buckets = self.buckets.lock().expect("rate limiter mutex poisoned");
        let now = Instant::now();
        let bucket = buckets.entry(ip).or_insert_with(|| Bucket {
            window_start: now,
            count: 0,
        });
        if now.duration_since(bucket.window_start) >= Duration::from_secs(60) {
            bucket.window_start = now;
            bucket.count = 0;
        }
        if bucket.count >= self.limit_per_min {
            return false;
        }
        bucket.count += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn allows_up_to_limit_then_blocks() {
        let rl = RateLimiter::new(2);
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        assert!(rl.allow(ip));
        assert!(rl.allow(ip));
        assert!(!rl.allow(ip));
    }

    #[test]
    fn zero_limit_means_unlimited() {
        let rl = RateLimiter::new(0);
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        for _ in 0..1000 {
            assert!(rl.allow(ip));
        }
    }

    #[test]
    fn different_ips_have_separate_buckets() {
        let rl = RateLimiter::new(1);
        let a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        assert!(rl.allow(a));
        assert!(rl.allow(b));
        assert!(!rl.allow(a));
    }
}
