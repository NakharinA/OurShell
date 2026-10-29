//! AI access policy: which hosts an AI session may touch and whether a human must approve.
//!
//! Rules that hold in every mode:
//! - Only hosts marked `ai_allowed` exist from the AI's point of view.
//! - Dangerous mode is scoped to one AI session, a subset of AI-allowed hosts, and a
//!   deadline. It never widens the set of reachable hosts.
//! - The kill switch (`stop_all`) ends dangerous mode, rejects pending approvals and
//!   blocks the session until it reconnects with a new id.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use ourshell_proto::agent::{AiSessionInfo, ApprovalRequest};
use tokio::sync::oneshot;

pub const DEFAULT_DANGEROUS_MINUTES: u64 = 60;
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
pub const DEFAULT_MAX_OUTPUT: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Run now without asking (dangerous mode for this session and host).
    Allow,
    /// Ask the human first.
    Ask,
    /// Never allowed.
    Deny(String),
}

#[derive(Default)]
struct SessionState {
    client_name: Option<String>,
    dangerous_hosts: HashSet<String>,
    dangerous_until: Option<DateTime<Utc>>,
    stopped: bool,
}

struct Pending {
    req: ApprovalRequest,
    tx: oneshot::Sender<bool>,
}

#[derive(Default)]
pub struct Policy {
    sessions: HashMap<String, SessionState>,
    pending: HashMap<String, Pending>,
}

impl Policy {
    pub fn register_session(&mut self, ai_session: &str, client_name: Option<String>) {
        let s = self.sessions.entry(ai_session.to_string()).or_default();
        if client_name.is_some() {
            s.client_name = client_name;
        }
    }

    pub fn end_session(&mut self, ai_session: &str) {
        self.sessions.remove(ai_session);
        let ids: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| p.req.ai_session == ai_session)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.resolve(&id, false);
        }
    }

    /// Decides what happens to a call. `ai_allowed` is the host's opt-in flag.
    pub fn decide(
        &mut self,
        ai_session: &str,
        host: &str,
        ai_allowed: bool,
        now: DateTime<Utc>,
    ) -> Decision {
        if !ai_allowed {
            return Decision::Deny(format!("host `{host}` is not available to AI"));
        }
        let Some(s) = self.sessions.get_mut(ai_session) else {
            return Decision::Ask;
        };
        if s.stopped {
            return Decision::Deny("AI access was stopped by the user (kill switch)".into());
        }
        if let Some(until) = s.dangerous_until {
            if now >= until {
                s.dangerous_until = None;
                s.dangerous_hosts.clear();
            } else if s.dangerous_hosts.contains(host) {
                return Decision::Allow;
            }
        }
        Decision::Ask
    }

    pub fn is_dangerous(&self, ai_session: &str, now: DateTime<Utc>) -> bool {
        self.sessions
            .get(ai_session)
            .and_then(|s| s.dangerous_until)
            .is_some_and(|u| now < u)
    }

    /// Turns dangerous mode on (minutes > 0) or off (0) for one session.
    /// `hosts` must already be filtered to AI-allowed hosts by the caller.
    pub fn set_dangerous(
        &mut self,
        ai_session: &str,
        hosts: Vec<String>,
        minutes: u64,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let s = self
            .sessions
            .get_mut(ai_session)
            .ok_or_else(|| format!("no AI session `{ai_session}`"))?;
        if s.stopped {
            return Err("this AI session was stopped; reconnect the AI client".into());
        }
        if minutes == 0 || hosts.is_empty() {
            s.dangerous_hosts.clear();
            s.dangerous_until = None;
        } else {
            s.dangerous_hosts = hosts.into_iter().collect();
            s.dangerous_until = Some(now + chrono::Duration::minutes(minutes as i64));
        }
        Ok(())
    }

    /// Kill switch.
    pub fn stop_all(&mut self) {
        for s in self.sessions.values_mut() {
            s.stopped = true;
            s.dangerous_hosts.clear();
            s.dangerous_until = None;
        }
        let ids: Vec<String> = self.pending.keys().cloned().collect();
        for id in ids {
            self.resolve(&id, false);
        }
    }

    pub fn session_client_name(&self, ai_session: &str) -> Option<String> {
        self.sessions
            .get(ai_session)
            .and_then(|s| s.client_name.clone())
    }

    /// Queues an approval and returns the receiver for the human's answer.
    pub fn request_approval(&mut self, req: ApprovalRequest) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        self.pending
            .insert(req.approval_id.clone(), Pending { req, tx });
        rx
    }

    /// Resolves a pending approval. Returns false if it no longer exists.
    pub fn resolve(&mut self, approval_id: &str, approve: bool) -> bool {
        match self.pending.remove(approval_id) {
            Some(p) => {
                let _ = p.tx.send(approve);
                true
            }
            None => false,
        }
    }

    pub fn pending(&self) -> Vec<ApprovalRequest> {
        let mut v: Vec<_> = self.pending.values().map(|p| p.req.clone()).collect();
        v.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        v
    }

    pub fn sessions(&self, now: DateTime<Utc>) -> Vec<AiSessionInfo> {
        let mut v: Vec<_> = self
            .sessions
            .iter()
            .map(|(id, s)| {
                let active = s.dangerous_until.is_some_and(|u| now < u);
                let mut hosts: Vec<_> = if active {
                    s.dangerous_hosts.iter().cloned().collect()
                } else {
                    vec![]
                };
                hosts.sort();
                AiSessionInfo {
                    ai_session: id.clone(),
                    ai_client_name: s.client_name.clone(),
                    dangerous_hosts: hosts,
                    dangerous_until: s.dangerous_until.filter(|_| active).map(|u| u.to_rfc3339()),
                }
            })
            .collect();
        v.sort_by(|a, b| a.ai_session.cmp(&b.ai_session));
        v
    }
}

/// Masks strings that look like secrets before output reaches the AI.
pub fn redact(text: &str) -> String {
    super::redact::redact(text)
}

/// Extra bytes fetched beyond the output cap so redaction sees a secret whole
/// before the cut; otherwise a secret split by the size limit could slip
/// through half-masked.
pub const REDACTION_SLACK: usize = 16 * 1024;

/// Truncates to at most `max` bytes on a char boundary. Returns (text, truncated).
pub fn cap(text: String, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text, false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn hidden_hosts_always_denied() {
        let mut p = Policy::default();
        p.register_session("s1", None);
        p.set_dangerous("s1", vec!["db".into()], 60, now()).unwrap();
        assert!(matches!(
            p.decide("s1", "db", false, now()),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn approval_by_default() {
        let mut p = Policy::default();
        p.register_session("s1", None);
        assert_eq!(p.decide("s1", "web", true, now()), Decision::Ask);
    }

    #[test]
    fn dangerous_mode_is_scoped_to_session_and_hosts() {
        let mut p = Policy::default();
        p.register_session("s1", None);
        p.register_session("s2", None);
        p.set_dangerous("s1", vec!["web".into()], 60, now())
            .unwrap();
        assert_eq!(p.decide("s1", "web", true, now()), Decision::Allow);
        assert_eq!(p.decide("s1", "db", true, now()), Decision::Ask);
        assert_eq!(p.decide("s2", "web", true, now()), Decision::Ask);
    }

    #[test]
    fn dangerous_mode_expires() {
        let mut p = Policy::default();
        p.register_session("s1", None);
        let t0 = now();
        p.set_dangerous("s1", vec!["web".into()], 1, t0).unwrap();
        assert_eq!(
            p.decide("s1", "web", true, t0 + chrono::Duration::seconds(30)),
            Decision::Allow
        );
        assert_eq!(
            p.decide("s1", "web", true, t0 + chrono::Duration::seconds(61)),
            Decision::Ask
        );
        assert!(!p.is_dangerous("s1", t0 + chrono::Duration::seconds(61)));
    }

    #[test]
    fn kill_switch_denies_and_rejects_pending() {
        let mut p = Policy::default();
        p.register_session("s1", None);
        p.set_dangerous("s1", vec!["web".into()], 60, now())
            .unwrap();
        let mut rx = p.request_approval(ApprovalRequest {
            approval_id: "a1".into(),
            ai_session: "s1".into(),
            ai_client_name: None,
            host: "web".into(),
            tool: "run_command".into(),
            detail: "ls".into(),
            created_at: now().to_rfc3339(),
        });
        p.stop_all();
        assert_eq!(rx.try_recv(), Ok(false));
        assert!(matches!(
            p.decide("s1", "web", true, now()),
            Decision::Deny(_)
        ));
        assert!(p
            .set_dangerous("s1", vec!["web".into()], 60, now())
            .is_err());
    }

    #[test]
    fn approve_and_reject() {
        let mut p = Policy::default();
        let req = |id: &str| ApprovalRequest {
            approval_id: id.into(),
            ai_session: "s".into(),
            ai_client_name: None,
            host: "h".into(),
            tool: "t".into(),
            detail: "d".into(),
            created_at: now().to_rfc3339(),
        };
        let mut a = p.request_approval(req("a"));
        let mut b = p.request_approval(req("b"));
        assert_eq!(p.pending().len(), 2);
        assert!(p.resolve("a", true));
        assert!(p.resolve("b", false));
        assert!(!p.resolve("b", true));
        assert_eq!(a.try_recv(), Ok(true));
        assert_eq!(b.try_recv(), Ok(false));
    }

    #[test]
    fn capping() {
        assert_eq!(cap("hello".into(), 10), ("hello".into(), false));
        assert_eq!(cap("héllo".into(), 2), ("h".into(), true));
    }
}
