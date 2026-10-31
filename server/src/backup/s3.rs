//! A minimal client for S3-compatible object storage: PUT / GET / HEAD /
//! DELETE object and ListObjectsV2, signed with AWS Signature Version 4.
//!
//! Hand-rolled (hmac + sha2 + reqwest) rather than the AWS SDK, which
//! would drag in aws-lc-rs (see "Dependency note" in docs/self-hosting.md).
//! Works with AWS S3, Cloudflare R2, Backblaze B2, Wasabi, MinIO, Synology
//! C2 and other S3-compatible stores, virtual-hosted or path-style.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use reqwest::{Method, StatusCode, Url};
use sha2::{Digest, Sha256};

use super::format::hex;

/// Where backups go. `endpoint` empty = AWS S3 in `region`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct S3Config {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub path_style: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Object {
    pub key: String,
    pub size: u64,
    pub last_modified: Option<DateTime<Utc>>,
}

pub struct S3Client {
    http: reqwest::Client,
    cfg: S3Config,
    /// `https://host[:port]`, no path.
    scheme: String,
    host: String,
}

#[cfg(test)]
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

impl S3Client {
    pub fn new(cfg: S3Config) -> Result<Self> {
        if cfg.bucket.trim().is_empty() {
            bail!("no bucket is set");
        }
        if cfg.access_key_id.is_empty() || cfg.secret_access_key.is_empty() {
            bail!("the access key ID and secret access key are required");
        }
        let endpoint = match cfg.endpoint.trim() {
            "" => format!("https://s3.{}.amazonaws.com", cfg.region),
            e if e.contains("://") => e.to_string(),
            e => format!("https://{e}"),
        };
        let url =
            Url::parse(&endpoint).with_context(|| format!("invalid endpoint {endpoint:?}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            bail!("the endpoint must be an http:// or https:// URL");
        }
        let host = url
            .host_str()
            .ok_or_else(|| anyhow!("the endpoint {endpoint:?} has no host"))?;
        let host = match url.port() {
            Some(p) => format!("{host}:{p}"),
            None => host.to_string(),
        };
        let host = if cfg.path_style {
            host
        } else {
            format!("{}.{host}", cfg.bucket)
        };
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(600))
            .build()
            .context("failed to build the HTTP client")?;
        Ok(Self {
            http,
            scheme: url.scheme().to_string(),
            host,
            cfg,
        })
    }

    pub fn bucket(&self) -> &str {
        &self.cfg.bucket
    }

    fn path(&self, key: &str) -> String {
        let mut p = String::from("/");
        if self.cfg.path_style {
            p.push_str(&uri_encode(&self.cfg.bucket, false));
            if !key.is_empty() {
                p.push('/');
            }
        }
        p.push_str(&uri_encode(key, true));
        p
    }

    async fn send(
        &self,
        method: Method,
        key: &str,
        query: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<reqwest::Response> {
        let now = Utc::now();
        let payload_hash = hex(&Sha256::digest(&body));
        let path = self.path(key);
        let query: Vec<(String, String)> = query
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let headers = vec![
            ("host".to_string(), self.host.clone()),
            ("x-amz-content-sha256".to_string(), payload_hash.clone()),
            ("x-amz-date".to_string(), amz_date.clone()),
        ];
        let auth = authorization(&SignRequest {
            method: method.as_str(),
            canonical_uri: &path,
            query: &query,
            headers: &headers,
            payload_hash: &payload_hash,
            amz_date: &amz_date,
            region: &self.cfg.region,
            access_key_id: &self.cfg.access_key_id,
            secret_access_key: &self.cfg.secret_access_key,
        });
        let qs = canonical_query(&query);
        let url = format!(
            "{}://{}{path}{}{qs}",
            self.scheme,
            self.host,
            if qs.is_empty() { "" } else { "?" }
        );
        let url = Url::parse(&url).with_context(|| format!("bad request URL {url}"))?;
        let res = self
            .http
            .request(method, url)
            .header("x-amz-content-sha256", payload_hash)
            .header("x-amz-date", amz_date)
            .header("authorization", auth)
            .body(body)
            .send()
            .await
            .map_err(|e| anyhow!("can't reach the storage endpoint: {}", error_chain(&e)))?;
        Ok(res)
    }

    async fn ok(res: reqwest::Response) -> Result<reqwest::Response> {
        let status = res.status();
        if status.is_success() {
            return Ok(res);
        }
        let body = res.text().await.unwrap_or_default();
        Err(s3_error(status, &body))
    }

    pub async fn put_object(&self, key: &str, body: Vec<u8>) -> Result<()> {
        Self::ok(self.send(Method::PUT, key, &[], body).await?).await?;
        Ok(())
    }

    pub async fn get_object(&self, key: &str) -> Result<Vec<u8>> {
        let res = Self::ok(self.send(Method::GET, key, &[], Vec::new()).await?).await?;
        Ok(res.bytes().await?.to_vec())
    }

    /// `false` if the object doesn't exist.
    pub async fn object_exists(&self, key: &str) -> Result<bool> {
        let res = self.send(Method::HEAD, key, &[], Vec::new()).await?;
        match res.status() {
            s if s.is_success() => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            s => Err(s3_error(s, "")),
        }
    }

    pub async fn delete_object(&self, key: &str) -> Result<()> {
        Self::ok(self.send(Method::DELETE, key, &[], Vec::new()).await?).await?;
        Ok(())
    }

    /// Creates the bucket (path-style `PUT /bucket`); used by tests against
    /// a local MinIO.
    pub async fn create_bucket(&self) -> Result<()> {
        let res = self.send(Method::PUT, "", &[], Vec::new()).await?;
        if res.status() == StatusCode::CONFLICT {
            return Ok(()); // already exists
        }
        Self::ok(res).await?;
        Ok(())
    }

    /// Every object under `prefix` (ListObjectsV2, following pagination).
    pub async fn list(&self, prefix: &str) -> Result<Vec<Object>> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![("list-type", "2"), ("prefix", prefix)];
            if let Some(t) = &token {
                query.push(("continuation-token", t.as_str()));
            }
            let res = Self::ok(self.send(Method::GET, "", &query, Vec::new()).await?).await?;
            let xml = res.text().await?;
            for block in xml_blocks(&xml, "Contents") {
                let Some(key) = xml_value(block, "Key") else {
                    continue;
                };
                out.push(Object {
                    key,
                    size: xml_value(block, "Size")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0),
                    last_modified: xml_value(block, "LastModified")
                        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                        .map(|d| d.with_timezone(&Utc)),
                });
            }
            let truncated = xml_value(&xml, "IsTruncated").is_some_and(|v| v == "true");
            token = xml_value(&xml, "NextContinuationToken");
            if !truncated || token.is_none() {
                return Ok(out);
            }
        }
    }

    /// Checks the endpoint, credentials and bucket with a one-key listing.
    pub async fn test(&self, prefix: &str) -> Result<()> {
        let query = [("list-type", "2"), ("prefix", prefix), ("max-keys", "1")];
        Self::ok(self.send(Method::GET, "", &query, Vec::new()).await?).await?;
        Ok(())
    }
}

fn error_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(cause) = src {
        s.push_str(": ");
        s.push_str(&cause.to_string());
        src = cause.source();
    }
    s
}

fn s3_error(status: StatusCode, body: &str) -> anyhow::Error {
    let code = xml_value(body, "Code");
    let message = xml_value(body, "Message");
    let hint = match (status.as_u16(), code.as_deref()) {
        (_, Some("SignatureDoesNotMatch")) => " (check the secret access key and region)",
        (_, Some("InvalidAccessKeyId")) => " (check the access key ID)",
        (_, Some("NoSuchBucket")) | (404, None) => {
            " (check the bucket name, and try path-style addressing for MinIO or Synology C2)"
        }
        (301, _) | (_, Some("PermanentRedirect")) => {
            " (the bucket is in another region, or the endpoint is wrong)"
        }
        (_, Some("AuthorizationHeaderMalformed")) => " (check the region)",
        (403, None) | (_, Some("AccessDenied")) => " (the key isn't allowed to do this)",
        _ => "",
    };
    match (code, message) {
        (Some(c), Some(m)) => anyhow!("storage error {}: {c}: {m}{hint}", status.as_u16()),
        (Some(c), None) => anyhow!("storage error {}: {c}{hint}", status.as_u16()),
        _ => anyhow!("storage error {}{hint}", status.as_u16()),
    }
}

/// The first `<tag>...</tag>` text in `xml`, entity-decoded.
fn xml_value(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = start + xml[start..].find(&close)?;
    Some(xml_unescape(&xml[start..end]))
}

/// The inner text of every `<tag>...</tag>` element.
fn xml_blocks<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i + open.len()..];
        let Some(j) = after.find(&close) else { break };
        out.push(&after[..j]);
        rest = &after[j + close.len()..];
    }
    out
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

// ---- Signature Version 4 ----------------------------------------------

/// What gets signed. `canonical_uri` is already URI-encoded; `query` is
/// raw (encoded here); `headers` are lower-case names, all of them signed.
pub struct SignRequest<'a> {
    pub method: &'a str,
    pub canonical_uri: &'a str,
    pub query: &'a [(String, String)],
    pub headers: &'a [(String, String)],
    pub payload_hash: &'a str,
    /// `YYYYMMDDTHHMMSSZ`.
    pub amz_date: &'a str,
    pub region: &'a str,
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
}

/// S3's URI encoding: everything but `A-Za-z0-9-_.~` (and `/` in paths).
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn canonical_query(query: &[(String, String)]) -> String {
    let mut pairs: Vec<(String, String)> = query
        .iter()
        .map(|(k, v)| (uri_encode(k, false), uri_encode(v, false)))
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

pub fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> [u8; 32] {
    let k = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k = hmac(&k, region.as_bytes());
    let k = hmac(&k, service.as_bytes());
    hmac(&k, b"aws4_request")
}

/// `(canonical request, string to sign, signature)`, for tests.
pub fn sign(r: &SignRequest) -> (String, String, String) {
    let mut headers: Vec<(String, String)> = r
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.to_ascii_lowercase(),
                v.split_whitespace().collect::<Vec<_>>().join(" "),
            )
        })
        .collect();
    headers.sort();
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = headers
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let creq = format!(
        "{}\n{}\n{}\n{canonical_headers}\n{signed_headers}\n{}",
        r.method,
        r.canonical_uri,
        canonical_query(r.query),
        r.payload_hash
    );
    let date = &r.amz_date[..8];
    let scope = format!("{date}/{}/s3/aws4_request", r.region);
    let sts = format!(
        "AWS4-HMAC-SHA256\n{}\n{scope}\n{}",
        r.amz_date,
        hex(&Sha256::digest(creq.as_bytes()))
    );
    let key = signing_key(r.secret_access_key, date, r.region, "s3");
    let signature = hex(&hmac(&key, sts.as_bytes()));
    (creq, sts, signature)
}

/// The `Authorization` header value.
pub fn authorization(r: &SignRequest) -> String {
    let (_, _, signature) = sign(r);
    let mut names: Vec<String> = r
        .headers
        .iter()
        .map(|(k, _)| k.to_ascii_lowercase())
        .collect();
    names.sort();
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{}/{}/s3/aws4_request, SignedHeaders={}, Signature={signature}",
        r.access_key_id,
        &r.amz_date[..8],
        r.region,
        names.join(";")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // The worked examples from AWS's "Signature Calculations for the
    // Authorization Header" (S3 API reference, Signature Version 4).
    const AK: &str = "AKIAIOSFODNN7EXAMPLE";
    const SK: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    const HOST: &str = "examplebucket.s3.amazonaws.com";
    const DATE: &str = "20130524T000000Z";

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn req<'a>(
        method: &'a str,
        uri: &'a str,
        query: &'a [(String, String)],
        headers: &'a [(String, String)],
        payload_hash: &'a str,
    ) -> SignRequest<'a> {
        SignRequest {
            method,
            canonical_uri: uri,
            query,
            headers,
            payload_hash,
            amz_date: DATE,
            region: "us-east-1",
            access_key_id: AK,
            secret_access_key: SK,
        }
    }

    #[test]
    fn aws_example_get_object() {
        let headers = h(&[
            ("Host", HOST),
            ("Range", "bytes=0-9"),
            ("x-amz-content-sha256", EMPTY_SHA256),
            ("x-amz-date", DATE),
        ]);
        let r = req("GET", "/test.txt", &[], &headers, EMPTY_SHA256);
        let (creq, sts, sig) = sign(&r);
        assert_eq!(
            creq,
            "GET\n/test.txt\n\nhost:examplebucket.s3.amazonaws.com\nrange:bytes=0-9\n\
             x-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
             x-amz-date:20130524T000000Z\n\nhost;range;x-amz-content-sha256;x-amz-date\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sts,
            "AWS4-HMAC-SHA256\n20130524T000000Z\n20130524/us-east-1/s3/aws4_request\n\
             7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972"
        );
        assert_eq!(
            sig,
            "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
        assert_eq!(
            authorization(&r),
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn aws_example_put_object() {
        let body_hash = hex(&Sha256::digest(b"Welcome to Amazon S3."));
        assert_eq!(
            body_hash,
            "44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072"
        );
        let headers = h(&[
            ("Host", HOST),
            ("Date", "Fri, 24 May 2013 00:00:00 GMT"),
            ("x-amz-date", DATE),
            ("x-amz-storage-class", "REDUCED_REDUNDANCY"),
            ("x-amz-content-sha256", &body_hash),
        ]);
        let uri = format!("/{}", uri_encode("test$file.text", true));
        assert_eq!(uri, "/test%24file.text");
        let (_, _, sig) = sign(&req("PUT", &uri, &[], &headers, &body_hash));
        assert_eq!(
            sig,
            "98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
        );
    }

    #[test]
    fn aws_example_get_bucket_lifecycle_and_list() {
        let headers = h(&[
            ("Host", HOST),
            ("x-amz-content-sha256", EMPTY_SHA256),
            ("x-amz-date", DATE),
        ]);
        let q = h(&[("lifecycle", "")]);
        let (_, _, sig) = sign(&req("GET", "/", &q, &headers, EMPTY_SHA256));
        assert_eq!(
            sig,
            "fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"
        );
        // Query parameters are sorted by name.
        let q = h(&[("prefix", "J"), ("max-keys", "2")]);
        let (creq, _, sig) = sign(&req("GET", "/", &q, &headers, EMPTY_SHA256));
        assert!(creq.starts_with("GET\n/\nmax-keys=2&prefix=J\n"), "{creq}");
        assert_eq!(
            sig,
            "34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"
        );
    }

    #[test]
    fn aws_example_signing_key() {
        // From the general SigV4 docs ("Examples of how to derive a signing key").
        let k = signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20120215",
            "us-east-1",
            "iam",
        );
        assert_eq!(
            hex(&k),
            "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d"
        );
    }

    #[test]
    fn addressing_styles() {
        let cfg = S3Config {
            endpoint: "http://nas.local:9000".into(),
            region: "us-east-1".into(),
            bucket: "backups".into(),
            access_key_id: "a".into(),
            secret_access_key: "s".into(),
            path_style: true,
        };
        let c = S3Client::new(cfg.clone()).unwrap();
        assert_eq!(c.host, "nas.local:9000");
        assert_eq!(c.path("a b/x.osbk"), "/backups/a%20b/x.osbk");
        assert_eq!(c.path(""), "/backups");
        let c = S3Client::new(S3Config {
            endpoint: String::new(),
            region: "eu-west-1".into(),
            path_style: false,
            ..cfg
        })
        .unwrap();
        assert_eq!(c.scheme, "https");
        assert_eq!(c.host, "backups.s3.eu-west-1.amazonaws.com");
        assert_eq!(c.path("x.osbk"), "/x.osbk");
        assert_eq!(c.path(""), "/");
    }

    #[test]
    fn parses_list_responses() {
        let xml = r#"<?xml version="1.0"?><ListBucketResult><IsTruncated>true</IsTruncated>
            <Contents><Key>p/ourshell-backup-1.osbk</Key><LastModified>2026-11-01T02:03:04.000Z</LastModified><Size>123</Size></Contents>
            <Contents><Key>p/a&amp;b</Key><Size>7</Size></Contents>
            <NextContinuationToken>tok/+=</NextContinuationToken></ListBucketResult>"#;
        let blocks = xml_blocks(xml, "Contents");
        assert_eq!(blocks.len(), 2);
        assert_eq!(xml_value(blocks[1], "Key").unwrap(), "p/a&b");
        assert_eq!(xml_value(blocks[0], "Size").unwrap(), "123");
        assert_eq!(xml_value(xml, "NextContinuationToken").unwrap(), "tok/+=");
        let err = s3_error(
            StatusCode::FORBIDDEN,
            "<Error><Code>SignatureDoesNotMatch</Code><Message>nope</Message></Error>",
        );
        assert!(err.to_string().contains("SignatureDoesNotMatch: nope"));
    }
}
