//! Validation and remote-command building for the typed, read-only
//! Kubernetes AI tools (`ai.k8s_get`, `ai.k8s_describe`, `ai.k8s_logs`,
//! `ai.k8s_events`) and `ai.system_status`.
//!
//! The AI never supplies a shell command for these tools: it supplies a
//! handful of strictly validated tokens (kind, namespace, name, ...), and
//! the agent assembles the remote command itself from those tokens -- the
//! whole point being that no client-provided text ever becomes part of the
//! command except through this validation. Every token is still
//! shell-quoted before being sent, as defence in depth, even though it has
//! already been validated to a safe character set.

use std::fmt;

/// A problem building a read-only k8s command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum K8sError {
    /// A bad parameter; the caller maps this to `INVALID_PARAMS`.
    Invalid(String),
    /// `k8s_describe` was asked for a `configmap`; the caller maps this to
    /// `POLICY_DENIED` (describe prints the object's data, which may contain
    /// secret-like values -- `k8s_get` may still *list* configmaps by name).
    ConfigmapDescribeDenied,
}

impl fmt::Display for K8sError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            K8sError::Invalid(m) => write!(f, "{m}"),
            K8sError::ConfigmapDescribeDenied => write!(
                f,
                "configmaps can hold secret-like data, and `describe` prints it in full. \
                 Use `run_command` (which requires approval) if you really need \
                 `kubectl describe configmap`; `k8s_describe` refuses this kind."
            ),
        }
    }
}

type Result<T> = std::result::Result<T, K8sError>;

fn invalid(msg: impl Into<String>) -> K8sError {
    K8sError::Invalid(msg.into())
}

/// `(aliases, canonical name, namespaced?)`. Aliases and the canonical name
/// are matched case-insensitively against a lowercased input; the allowlist
/// itself is the only source of truth for what's reachable, so an unlisted
/// kind (including every spelling of `secret`/`secrets`) is simply unknown.
const KINDS: &[(&[&str], &str, bool)] = &[
    (&["po", "pod", "pods"], "pods", true),
    (
        &["deploy", "deployment", "deployments"],
        "deployments",
        true,
    ),
    (
        &["sts", "statefulset", "statefulsets"],
        "statefulsets",
        true,
    ),
    (&["ds", "daemonset", "daemonsets"], "daemonsets", true),
    (&["rs", "replicaset", "replicasets"], "replicasets", true),
    (&["svc", "service", "services"], "services", true),
    (&["ing", "ingress", "ingresses"], "ingresses", true),
    (&["no", "node", "nodes"], "nodes", false),
    (&["ns", "namespace", "namespaces"], "namespaces", false),
    (&["job", "jobs"], "jobs", true),
    (&["cj", "cronjob", "cronjobs"], "cronjobs", true),
    (
        &["pvc", "persistentvolumeclaim", "persistentvolumeclaims"],
        "persistentvolumeclaims",
        true,
    ),
    (
        &["pv", "persistentvolume", "persistentvolumes"],
        "persistentvolumes",
        false,
    ),
    (&["ev", "event", "events"], "events", true),
    (&["ep", "endpoint", "endpoints"], "endpoints", true),
    (&["cm", "configmap", "configmaps"], "configmaps", true),
];

/// Resolves a (possibly aliased, possibly mixed-case) kind to its canonical
/// name and whether it's namespaced. Anything not on the allowlist -- in
/// particular `secret`/`secrets` in any case -- is rejected.
fn resolve_kind(kind: &str) -> Result<(&'static str, bool)> {
    let k = kind.to_lowercase();
    KINDS
        .iter()
        .find(|(aliases, _, _)| aliases.contains(&k.as_str()))
        .map(|(_, canonical, namespaced)| (*canonical, *namespaced))
        .ok_or_else(|| {
            invalid(format!(
                "unknown or unsupported kind `{kind}`; see the tool description for the allowed kinds"
            ))
        })
}

fn is_label_char(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit()
}

/// `^[a-z0-9]([-a-z0-9.]*[a-z0-9])?$` (names, pods, containers).
fn is_dns_subdomain(s: &str) -> bool {
    let b = s.as_bytes();
    match b.len() {
        0 => false,
        1 => is_label_char(b[0]),
        _ => {
            is_label_char(b[0])
                && is_label_char(b[b.len() - 1])
                && b[1..b.len() - 1]
                    .iter()
                    .all(|&c| is_label_char(c) || c == b'-' || c == b'.')
        }
    }
}

/// `^[a-z0-9]([-a-z0-9]*[a-z0-9])?$` (namespaces).
fn is_dns_label(s: &str) -> bool {
    let b = s.as_bytes();
    match b.len() {
        0 => false,
        1 => is_label_char(b[0]),
        _ => {
            is_label_char(b[0])
                && is_label_char(b[b.len() - 1])
                && b[1..b.len() - 1]
                    .iter()
                    .all(|&c| is_label_char(c) || c == b'-')
        }
    }
}

fn validate_name(field: &str, s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 253 || !is_dns_subdomain(s) {
        return Err(invalid(format!(
            "`{field}` must be a valid Kubernetes name (lowercase alphanumeric, `-` or `.`, \
             starting and ending with alphanumeric, at most 253 characters); got `{s}`"
        )));
    }
    Ok(())
}

fn validate_namespace(ns: &str) -> Result<()> {
    if ns.is_empty() || ns.len() > 63 || !is_dns_label(ns) {
        return Err(invalid(format!(
            "`namespace` must be a valid Kubernetes namespace (lowercase alphanumeric or `-`, \
             starting and ending with alphanumeric, at most 63 characters); got `{ns}`"
        )));
    }
    Ok(())
}

/// Namespace presence rule: required for namespaced kinds, forbidden for
/// cluster-scoped ones (`nodes`, `namespaces`, `persistentvolumes`).
fn check_namespace_presence(
    canonical: &str,
    namespaced: bool,
    namespace: Option<&str>,
) -> Result<()> {
    match (namespaced, namespace) {
        (true, None) => Err(invalid(format!(
            "`namespace` is required for kind `{canonical}`"
        ))),
        (false, Some(_)) => Err(invalid(format!(
            "kind `{canonical}` is cluster-scoped; do not pass `namespace`"
        ))),
        _ => Ok(()),
    }
}

/// Maximum `--tail` the agent will ever pass, regardless of what's asked for.
pub const TAIL_MAX: u32 = 500;
const TAIL_DEFAULT: u32 = 200;
/// Maximum `--since` duration, in seconds (7 days).
const SINCE_MAX_SECS: u64 = 7 * 24 * 3600;

fn clamp_tail(tail: Option<u32>) -> u32 {
    tail.unwrap_or(TAIL_DEFAULT).min(TAIL_MAX)
}

/// A duration like `30s`, `15m`, `2h`, `1d`: 1-4 digits followed by one of
/// `s`/`m`/`h`/`d`, at most 7 days in total.
fn validate_since(since: &str) -> Result<()> {
    let bad = || {
        invalid(format!(
            "`since` must look like `30s`, `15m`, `2h` or `1d` (1-4 digits plus a unit, at most 7 days); got `{since}`"
        ))
    };
    if since.len() < 2 || since.len() > 5 {
        return Err(bad());
    }
    let (digits, unit) = since.split_at(since.len() - 1);
    if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let per_sec: u64 = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err(bad()),
    };
    let n: u64 = digits.parse().map_err(|_| bad())?;
    if n == 0 || n.saturating_mul(per_sec) > SINCE_MAX_SECS {
        return Err(bad());
    }
    Ok(())
}

/// Shell-quotes a token with single quotes. Every token built into a command
/// goes through this, even though each one was already validated to a safe
/// character set -- defence in depth, not the primary protection.
fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Builds `kubectl get <kind> [-n <namespace>] [name] -o wide`.
pub fn get_command(kind: &str, namespace: Option<&str>, name: Option<&str>) -> Result<String> {
    let (canonical, namespaced) = resolve_kind(kind)?;
    check_namespace_presence(canonical, namespaced, namespace)?;
    if let Some(ns) = namespace {
        validate_namespace(ns)?;
    }
    if let Some(n) = name {
        validate_name("name", n)?;
    }
    let mut cmd = format!("kubectl get {}", q(canonical));
    if let Some(ns) = namespace {
        cmd.push_str(&format!(" -n {}", q(ns)));
    }
    if let Some(n) = name {
        cmd.push_str(&format!(" {}", q(n)));
    }
    cmd.push_str(" -o wide");
    Ok(cmd)
}

/// Builds `kubectl describe <kind> <name> [-n <namespace>]`.
pub fn describe_command(kind: &str, namespace: Option<&str>, name: &str) -> Result<String> {
    let (canonical, namespaced) = resolve_kind(kind)?;
    if canonical == "configmaps" {
        return Err(K8sError::ConfigmapDescribeDenied);
    }
    check_namespace_presence(canonical, namespaced, namespace)?;
    if let Some(ns) = namespace {
        validate_namespace(ns)?;
    }
    validate_name("name", name)?;
    let mut cmd = format!("kubectl describe {} {}", q(canonical), q(name));
    if let Some(ns) = namespace {
        cmd.push_str(&format!(" -n {}", q(ns)));
    }
    Ok(cmd)
}

/// Builds `kubectl logs <pod> -n <namespace> [-c <container>] --tail=N [--since=D]`.
pub fn logs_command(
    namespace: &str,
    pod: &str,
    container: Option<&str>,
    tail: Option<u32>,
    since: Option<&str>,
) -> Result<String> {
    validate_namespace(namespace)?;
    validate_name("pod", pod)?;
    if let Some(c) = container {
        validate_name("container", c)?;
    }
    if let Some(s) = since {
        validate_since(s)?;
    }
    let tail = clamp_tail(tail);
    let mut cmd = format!("kubectl logs {} -n {}", q(pod), q(namespace));
    if let Some(c) = container {
        cmd.push_str(&format!(" -c {}", q(c)));
    }
    cmd.push_str(&format!(" --tail={tail}"));
    if let Some(s) = since {
        cmd.push_str(&format!(" --since={}", q(s)));
    }
    Ok(cmd)
}

/// Builds `kubectl get events -n <namespace> --sort-by=.lastTimestamp`.
pub fn events_command(namespace: &str) -> Result<String> {
    validate_namespace(namespace)?;
    Ok(format!(
        "kubectl get events -n {} --sort-by=.lastTimestamp",
        q(namespace)
    ))
}

/// The fixed `system_status` command. Takes no arguments at all.
pub fn system_status_command() -> &'static str {
    "uptime; df -h; free -m"
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- exact built commands -----

    #[test]
    fn get_namespaced_with_name() {
        assert_eq!(
            get_command("pods", Some("default"), Some("nginx")).unwrap(),
            "kubectl get 'pods' -n 'default' 'nginx' -o wide"
        );
    }

    #[test]
    fn get_namespaced_list() {
        assert_eq!(
            get_command("deploy", Some("prod"), None).unwrap(),
            "kubectl get 'deployments' -n 'prod' -o wide"
        );
    }

    #[test]
    fn get_cluster_scoped() {
        assert_eq!(
            get_command("no", None, Some("node-1")).unwrap(),
            "kubectl get 'nodes' 'node-1' -o wide"
        );
        assert_eq!(
            get_command("ns", None, None).unwrap(),
            "kubectl get 'namespaces' -o wide"
        );
    }

    #[test]
    fn get_configmaps_allowed() {
        assert_eq!(
            get_command("configmap", Some("demo"), Some("backend-cfm")).unwrap(),
            "kubectl get 'configmaps' -n 'demo' 'backend-cfm' -o wide"
        );
    }

    #[test]
    fn describe_namespaced() {
        assert_eq!(
            describe_command("pod", Some("default"), "nginx-abc").unwrap(),
            "kubectl describe 'pods' 'nginx-abc' -n 'default'"
        );
    }

    #[test]
    fn describe_cluster_scoped() {
        assert_eq!(
            describe_command("node", None, "node-1").unwrap(),
            "kubectl describe 'nodes' 'node-1'"
        );
    }

    #[test]
    fn describe_configmap_denied() {
        let e = describe_command("cm", Some("default"), "backend-cfm").unwrap_err();
        assert_eq!(e, K8sError::ConfigmapDescribeDenied);
        let text = e.to_string();
        assert!(text.contains("run_command"));
    }

    #[test]
    fn logs_minimal() {
        assert_eq!(
            logs_command("default", "nginx-abc", None, None, None).unwrap(),
            "kubectl logs 'nginx-abc' -n 'default' --tail=200"
        );
    }

    #[test]
    fn logs_full() {
        assert_eq!(
            logs_command("default", "nginx-abc", Some("app"), Some(50), Some("15m")).unwrap(),
            "kubectl logs 'nginx-abc' -n 'default' -c 'app' --tail=50 --since='15m'"
        );
    }

    #[test]
    fn events_command_exact() {
        assert_eq!(
            events_command("demo").unwrap(),
            "kubectl get events -n 'demo' --sort-by=.lastTimestamp"
        );
    }

    #[test]
    fn system_status_is_fixed() {
        assert_eq!(system_status_command(), "uptime; df -h; free -m");
    }

    // ----- kind allowlist / aliases / case / secrets -----

    #[test]
    fn aliases_map_to_canonical() {
        for (alias, canonical) in [
            ("po", "pods"),
            ("pod", "pods"),
            ("deploy", "deployments"),
            ("sts", "statefulsets"),
            ("ds", "daemonsets"),
            ("rs", "replicasets"),
            ("svc", "services"),
            ("ing", "ingresses"),
            ("no", "nodes"),
            ("ns", "namespaces"),
            ("job", "jobs"),
            ("cj", "cronjobs"),
            ("pvc", "persistentvolumeclaims"),
            ("pv", "persistentvolumes"),
            ("ev", "events"),
            ("ep", "endpoints"),
            ("cm", "configmaps"),
        ] {
            let (c, _) = resolve_kind(alias).unwrap();
            assert_eq!(c, canonical, "alias {alias}");
        }
    }

    #[test]
    fn secret_rejected_in_every_case() {
        for s in ["secret", "secrets", "Secret", "SECRETS", "SeCrEt"] {
            assert!(matches!(resolve_kind(s), Err(K8sError::Invalid(_))), "{s}");
        }
    }

    #[test]
    fn unknown_kind_rejected() {
        assert!(resolve_kind("widgets").is_err());
        assert!(resolve_kind("").is_err());
    }

    #[test]
    fn kind_case_insensitive_for_valid_kinds() {
        let (c, _) = resolve_kind("PODS").unwrap();
        assert_eq!(c, "pods");
        let (c, _) = resolve_kind("Deploy").unwrap();
        assert_eq!(c, "deployments");
    }

    // ----- cluster-scoped namespace rules -----

    #[test]
    fn namespace_required_for_namespaced_kind() {
        assert!(get_command("pods", None, None).is_err());
        assert!(describe_command("svc", None, "x").is_err());
    }

    #[test]
    fn namespace_forbidden_for_cluster_scoped_kind() {
        assert!(get_command("node", Some("default"), None).is_err());
        assert!(describe_command("pv", Some("default"), "x").is_err());
    }

    // ----- injection attempts on namespace / name / pod / container -----

    fn bad_tokens() -> Vec<&'static str> {
        vec![
            "ns; rm -rf /",
            "$(id)",
            "`id`",
            "ns bad",
            "ns'bad",
            "ns\"bad",
            "ns\nbad",
            "Uppercase",
            "",
            "-leading",
            "trailing-",
        ]
    }

    #[test]
    fn injection_attempts_rejected_in_namespace() {
        for t in bad_tokens() {
            assert!(
                validate_namespace(t).is_err(),
                "namespace accepted bad token {t:?}"
            );
        }
    }

    #[test]
    fn injection_attempts_rejected_in_name() {
        for t in bad_tokens() {
            assert!(
                validate_name("name", t).is_err(),
                "name accepted bad token {t:?}"
            );
        }
    }

    #[test]
    fn overlong_namespace_rejected() {
        let ns = "a".repeat(64);
        assert!(validate_namespace(&ns).is_err());
        let ns = "a".repeat(63);
        assert!(validate_namespace(&ns).is_ok());
    }

    #[test]
    fn overlong_name_rejected() {
        let name = "a".repeat(254);
        assert!(validate_name("name", &name).is_err());
        let name = "a".repeat(253);
        assert!(validate_name("name", &name).is_ok());
    }

    #[test]
    fn dots_allowed_in_names_not_namespaces() {
        assert!(validate_name("name", "my.pod.1").is_ok());
        assert!(validate_namespace("my.ns").is_err());
    }

    // ----- tail / since bounds -----

    #[test]
    fn tail_defaults_and_clamps() {
        assert_eq!(
            logs_command("ns", "pod", None, None, None).unwrap(),
            "kubectl logs 'pod' -n 'ns' --tail=200"
        );
        assert_eq!(
            logs_command("ns", "pod", None, Some(10_000), None).unwrap(),
            "kubectl logs 'pod' -n 'ns' --tail=500"
        );
        assert_eq!(
            logs_command("ns", "pod", None, Some(10), None).unwrap(),
            "kubectl logs 'pod' -n 'ns' --tail=10"
        );
    }

    #[test]
    fn since_accepts_valid_durations() {
        for s in ["1s", "30s", "15m", "2h", "1d", "7d", "9999s"] {
            assert!(
                logs_command("ns", "pod", None, None, Some(s)).is_ok(),
                "{s}"
            );
        }
    }

    #[test]
    fn since_rejects_bad_formats_and_too_long() {
        for s in ["", "0s", "30x", "bad", "99999m", "8d", "1w", " 1d", "1d "] {
            assert!(
                logs_command("ns", "pod", None, None, Some(s)).is_err(),
                "{s} should be rejected"
            );
        }
    }

    #[test]
    fn since_boundary_seven_days() {
        assert!(logs_command("ns", "pod", None, None, Some("7d")).is_ok());
        assert!(logs_command("ns", "pod", None, None, Some("168h")).is_ok());
        assert!(logs_command("ns", "pod", None, None, Some("169h")).is_err());
    }
}
