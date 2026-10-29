//! Masks secrets in command output and files before they reach an AI client.
//!
//! Defence in depth only: approval mode is the main protection. The rules are
//! deliberately aggressive (a false positive costs the AI some context; a false
//! negative leaks a credential), and each one is written so a match can never
//! run across unrelated text and swallow the surrounding structure.

use std::sync::OnceLock;

use regex::{Captures, Regex};

pub const MASK: &str = "[REDACTED]";

/// Words that make a key name secret-bearing, e.g. `JWT_SECRET`, `R2_SECRET_ACCESS_KEY`,
/// `db_password`, `GITHUB_TOKEN`, `FIREBASE_PRIVATE_KEY`, `x-api-key`.
const SECRET_NAME: &str = r"[A-Za-z0-9_.\-]*(?:secret|passw(?:or)?d|pwd|token|private[_\-]?key|api[_\-]?key|access[_\-]?key|credential|auth[_\-]?key|client[_\-]?key|signing[_\-]?key|session[_\-]?key|encryption[_\-]?key)[A-Za-z0-9_.\-]*";

struct Rules {
    pem: Regex,
    url_creds: Regex,
    block_header: Regex,
    assignment: Regex,
    keyish_entropy: Regex,
    tokens: Vec<Regex>,
}

fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| {
        let re = |p: &str| Regex::new(p).expect("valid redaction regex");
        Rules {
            // A PEM private key. The body may only contain base64, whitespace and
            // escapes (`\n`, YAML line folding), so it can never run past the end
            // of one key into unrelated text; the END marker may be split by
            // whitespace or a folded line. A key cut off without END is masked
            // up to the first non-base64 character.
            pem: re(concat!(
                r"-----BEGIN[A-Z0-9 \\\r\n\t]*?PRIVATE[ \\\r\n\t]*KEY-----",
                r"[A-Za-z0-9+/=\\\s]*",
                r"(?:-----[ \\\r\n\t]*END[A-Z0-9 \\\r\n\t]*?KEY-----)?",
            )),
            // scheme://user:PASSWORD@host
            url_creds: re(r#"(?i)\b([a-z][a-z0-9+.\-]{1,20}://[^\s/:@'"]*:)([^\s@/'"]+)@"#),
            // YAML block scalar under a secret-named key: `JWT_SECRET: |`
            block_header: re(&format!(
                r#"(?i)^([ \t]*)(?:-[ \t]+)?\\?['"]?{SECRET_NAME}\\?['"]?[ \t]*:[ \t]*[|>][-+0-9]*[ \t]*$"#
            )),
            // NAME=value, NAME: value, "NAME": "value", \"NAME\":\"value\"
            assignment: re(&format!(
                r#"(?i)(\\?["']?{SECRET_NAME}\\?["']?[ \t]*[:=][ \t]*)("(?:[^"\\\r\n]|\\.)*"|'[^'\r\n]*'|[^\s,;}}&\r\n]+)"#
            )),
            // Long hex / base64 values on any key-ish name (e.g. `R2_KEY: <64 hex>`).
            keyish_entropy: re(
                r#"(?i)([A-Za-z0-9_.\-]*(?:key|secret|token|sig)[A-Za-z0-9_.\-]*\\?["']?[ \t]*[:=][ \t]*\\?["']?)([A-Fa-f0-9]{32,}|[A-Za-z0-9+/_\-]{40,}={0,2})"#,
            ),
            tokens: [
                r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
                r"\bgh[pousr]_[A-Za-z0-9]{30,}\b",
                r"\bgithub_pat_[A-Za-z0-9_]{40,}\b",
                r"\bglpat-[A-Za-z0-9_\-]{20,}\b",
                r"\bxox[abprs]-[A-Za-z0-9\-]{10,}\b",
                r"\bsk-(?:ant-)?[A-Za-z0-9_\-]{20,}\b",
                r"\bsk_(?:live|test)_[A-Za-z0-9]{16,}\b",
                r"\bAIza[0-9A-Za-z_\-]{35}\b",
                r"\beyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\b",
            ]
            .iter()
            .map(|p| re(p))
            .collect(),
        }
    })
}

/// Replaces the value of a matched assignment, keeping its quoting style.
fn mask_value(caps: &Captures) -> String {
    let value = &caps[2];
    if value.contains(MASK) {
        return caps[0].to_string();
    }
    let masked = match value.chars().next() {
        Some('"') => format!("\"{MASK}\""),
        Some('\'') => format!("'{MASK}'"),
        _ if value.starts_with("\\\"") => format!("\\\"{MASK}\\\""),
        _ => MASK.to_string(),
    };
    format!("{}{masked}", &caps[1])
}

/// Masks the indented lines that make up a secret-named YAML block scalar.
fn mask_block_scalars(text: &str, header: &Regex) -> String {
    let mut out = String::with_capacity(text.len());
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        out.push_str(line);
        let body = line.trim_end_matches(['\r', '\n']);
        let Some(caps) = header.captures(body) else {
            continue;
        };
        let indent = caps[1].len();
        let mut masked_any = false;
        while let Some(next) = lines.peek() {
            let content = next.trim_end_matches(['\r', '\n']);
            let next_indent = content.len() - content.trim_start().len();
            if !content.trim().is_empty() && next_indent <= indent {
                break;
            }
            if !masked_any && !content.trim().is_empty() {
                out.push_str(&" ".repeat(next_indent));
                out.push_str(MASK);
                out.push('\n');
                masked_any = true;
            }
            lines.next();
        }
    }
    out
}

/// Masks strings that look like secrets.
pub fn redact(text: &str) -> String {
    let r = rules();
    let mut out = r.pem.replace_all(text, MASK).into_owned();
    out = r
        .url_creds
        .replace_all(&out, format!("${{1}}{MASK}@"))
        .into_owned();
    out = mask_block_scalars(&out, &r.block_header);
    out = r.assignment.replace_all(&out, mask_value).into_owned();
    out = r
        .keyish_entropy
        .replace_all(&out, format!("${{1}}{MASK}"))
        .into_owned();
    for t in &r.tokens {
        out = t.replace_all(&out, MASK).into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every value below is a made-up fixture.
    const PEM_BODY: &str = "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7fakefakefake";
    const DB_PASS: &str = "s3cr3t-Pa55w0rd";
    const R2_SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const R2_ID: &str = "fedcba9876543210fedcba9876543210";
    const JWT_SECRET: &str = "super-long-jwt-signing-string";
    const API_KEY: &str = "AIzaSyFAKEfakeFAKEfakeFAKEfakeFAKEfak12";

    fn configmap() -> String {
        let pem_escaped = format!(
            "-----BEGIN PRIVATE KEY-----\\n{PEM_BODY}\\n{PEM_BODY}\\n-----END PRIVATE KEY-----\\n"
        );
        // kubectl folds long double-quoted YAML strings at spaces, splitting the
        // END marker across lines; the annotation repeats everything as JSON.
        format!(
            r#"apiVersion: v1
data:
  FIREBASE_API_KEY: {API_KEY}
  FIREBASE_PRIVATE_KEY: "-----BEGIN PRIVATE KEY-----\n{PEM_BODY}\n{PEM_BODY}\n-----END
    PRIVATE KEY-----\n"
  FIREBASE_PROJECT_ID: demo-project
  JWT_SECRET: {JWT_SECRET}
  POSTGRES_URL: postgresql://app_user:{DB_PASS}@db.internal:5432/appdb
  R2_ACCESS_KEY_ID: {R2_ID}
  R2_SECRET_ACCESS_KEY: {R2_SECRET}
kind: ConfigMap
metadata:
  annotations:
    kubectl.kubernetes.io/last-applied-configuration: |
      {{"apiVersion":"v1","data":{{"FIREBASE_API_KEY":"{API_KEY}","FIREBASE_PRIVATE_KEY":"{pem_escaped}","FIREBASE_PROJECT_ID":"demo-project","JWT_SECRET":"{JWT_SECRET}","POSTGRES_URL":"postgresql://app_user:{DB_PASS}@db.internal:5432/appdb","R2_ACCESS_KEY_ID":"{R2_ID}","R2_SECRET_ACCESS_KEY":"{R2_SECRET}"}},"kind":"ConfigMap","metadata":{{"name":"backend-cfm","namespace":"demo"}}}}
  name: backend-cfm
  namespace: demo
"#
        )
    }

    #[test]
    fn configmap_with_annotation_leaks_nothing() {
        let out = redact(&configmap());
        for secret in [PEM_BODY, DB_PASS, R2_SECRET, R2_ID, JWT_SECRET, API_KEY] {
            assert!(!out.contains(secret), "leaked {secret:?} in:\n{out}");
        }
        // Structure and non-secret values survive.
        for keep in [
            "metadata:",
            "annotations:",
            "last-applied-configuration",
            "FIREBASE_PROJECT_ID: demo-project",
            "\"FIREBASE_PROJECT_ID\":\"demo-project\"",
            "db.internal:5432/appdb",
            "name: backend-cfm",
            "namespace: demo",
        ] {
            assert!(out.contains(keep), "lost {keep:?} in:\n{out}");
        }
    }

    #[test]
    fn two_pem_blocks_keep_the_text_between_them() {
        let text = format!(
            "-----BEGIN RSA PRIVATE KEY-----\n{PEM_BODY}\n-----END RSA PRIVATE KEY-----\n\
             keep: this line, and this one\n\
             -----BEGIN OPENSSH PRIVATE KEY-----\n{PEM_BODY}\n-----END OPENSSH PRIVATE KEY-----\n"
        );
        let out = redact(&text);
        assert!(!out.contains(PEM_BODY), "{out}");
        assert!(out.contains("keep: this line, and this one"), "{out}");
        assert_eq!(out.matches(MASK).count(), 2, "{out}");
    }

    #[test]
    fn truncated_pem_is_still_masked() {
        let out = redact(&format!("x\n-----BEGIN PRIVATE KEY-----\n{PEM_BODY}"));
        assert!(!out.contains(PEM_BODY));
        assert!(out.starts_with("x\n"));
    }

    #[test]
    fn url_passwords() {
        for url in [
            format!("postgresql://u:{DB_PASS}@h:5432/db"),
            format!("mysql://root:{DB_PASS}@127.0.0.1/x"),
            format!("mongodb+srv://u:{DB_PASS}@cluster0.example.net/db"),
            format!("redis://:{DB_PASS}@cache:6379"),
            format!("amqp://guest:{DB_PASS}@mq/"),
            format!("https://user:{DB_PASS}@example.com/path"),
        ] {
            let out = redact(&url);
            assert!(!out.contains(DB_PASS), "{url} -> {out}");
            assert!(out.contains(MASK), "{out}");
        }
        // Ports and plain URLs are untouched.
        assert_eq!(
            redact("http://example.com:8080/a@b"),
            "http://example.com:8080/a@b"
        );
    }

    #[test]
    fn secret_named_keys_in_every_syntax() {
        let cases = [
            format!("JWT_SECRET={JWT_SECRET}"),
            format!("export AWS_SECRET_ACCESS_KEY='{R2_SECRET}'"),
            format!("db_password: \"{DB_PASS}\""),
            format!("{{\"R2_SECRET_ACCESS_KEY\": \"{R2_SECRET}\"}}"),
            format!("{{\\\"JWT_SECRET\\\":\\\"{JWT_SECRET}\\\"}}"),
            format!("GITHUB_TOKEN: {JWT_SECRET}"),
            format!("client-credentials={JWT_SECRET}"),
        ];
        for c in &cases {
            let out = redact(c);
            for s in [JWT_SECRET, R2_SECRET, DB_PASS] {
                assert!(!out.contains(s), "{c} -> {out}");
            }
        }
    }

    #[test]
    fn yaml_block_scalar_under_secret_key() {
        let text = format!(
            "data:\n  JWT_SECRET: |\n    {JWT_SECRET}\n    second-line\n  other: visible\n"
        );
        let out = redact(&text);
        assert!(
            !out.contains(JWT_SECRET) && !out.contains("second-line"),
            "{out}"
        );
        assert!(out.contains("other: visible"), "{out}");
    }

    #[test]
    fn high_entropy_values_on_key_names() {
        let out = redact(&format!("R2_KEY: {R2_SECRET}\nSIGNATURE={R2_ID}"));
        assert!(!out.contains(R2_SECRET) && !out.contains(R2_ID), "{out}");
    }

    #[test]
    fn ordinary_output_is_untouched() {
        let text = "NAME                     READY   STATUS    RESTARTS   AGE\n\
                    backend-6d8f9c7b5-x2x9z   1/1     Running   0          3d\n\
                    total 12\ndrwxr-xr-x 2 root root 4096 Oct 29 key-files\n";
        assert_eq!(redact(text), text);
    }

    #[test]
    fn known_token_formats() {
        for t in [
            "AKIAABCDEFGHIJKLMNOP",
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "sk-ant-abcdefghijklmnopqrstuvwxyz",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N",
        ] {
            assert_eq!(redact(&format!("x {t} y")), format!("x {MASK} y"), "{t}");
        }
    }
}
