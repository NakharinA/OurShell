//! `ourshell update` [`--check`]: self-updates the `ourshell` (and, if present
//! alongside it, `ourshell-mcp`) binary from the project's GitHub releases.
//!
//! Design notes (see the module's functions for the pieces this stitches
//! together):
//!
//! - The only source of updates is
//!   `https://api.github.com/repos/NakharinA/OurShell/releases/latest`.
//!   Nothing else is ever queried or trusted.
//! - Download, verify and extract are kept as separate, independently
//!   testable functions so the unit tests below can exercise checksum
//!   parsing/mismatch and archive extraction purely against local files,
//!   without a network call.
//! - Every downloaded URL is checked against an allowlist of GitHub asset
//!   hosts *before* it is requested, and the final (post-redirect) URL is
//!   checked again, so a compromised or misconfigured redirect can't smuggle
//!   in an arbitrary host.
//! - The archive's SHA-256 is verified against the release's own
//!   `SHA256SUMS-<target>.txt` before anything inside it is extracted or run.
//!   A mismatch is a hard error; nothing downloaded is ever executed.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use ourshell_proto::agent::{method, ClientKind};
use serde::Deserialize;
use serde_json::json;

use crate::client::{hello, AgentClient};

/// The one and only place updates come from. Hard-coded (not configurable at
/// runtime) by design: `ourshell update` must never be pointed at a
/// different host, e.g. by an environment variable an attacker controls.
const REPO_API_LATEST_RELEASE: &str =
    "https://api.github.com/repos/NakharinA/OurShell/releases/latest";

/// Exit code `--check` uses when a newer release exists, so scripts can act
/// on it without parsing output (documented in `ourshell update --help` and
/// the README).
pub const EXIT_UPDATE_AVAILABLE: i32 = 10;

/// Asset URLs are only ever fetched if they start with one of these. The
/// first is what GitHub's API hands back as `browser_download_url`; the
/// other two are where that URL redirects to when actually downloaded
/// (reqwest follows redirects transparently, so the final URL is checked
/// against this same list -- see `download_to`).
const ALLOWED_URL_PREFIXES: &[&str] = &[
    "https://github.com/NakharinA/OurShell/releases/download/",
    "https://objects.githubusercontent.com/",
    "https://release-assets.githubusercontent.com/",
];

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Everything `run_update`/`run_check` need from the GitHub API response, with
/// the asset list exposed for `pick_asset`.
pub struct ReleaseInfo {
    pub tag: String,
    pub html_url: String,
    pub notes: String,
    assets: Vec<GithubAsset>,
}

/// GETs the latest release. Returns `Ok(None)` if the repository has no
/// releases yet (the API returns 404 for that, which is expected right after
/// a fresh clone/install and must not be treated as an error).
pub async fn fetch_latest_release(client: &reqwest::Client) -> Result<Option<ReleaseInfo>> {
    let mut req = client
        .get(REPO_API_LATEST_RELEASE)
        .header("User-Agent", user_agent())
        .header("Accept", "application/vnd.github+json");
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
    }
    let resp = req
        .send()
        .await
        .context("could not reach GitHub (api.github.com)")?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        bail!(
            "GitHub API returned {} fetching the latest release{}",
            resp.status(),
            rate_limit_hint(&resp)
        );
    }
    let release: GithubRelease = resp
        .json()
        .await
        .context("could not parse GitHub API response")?;
    Ok(Some(ReleaseInfo {
        tag: release.tag_name,
        html_url: release.html_url,
        notes: release.body.unwrap_or_default(),
        assets: release.assets,
    }))
}

fn rate_limit_hint(resp: &reqwest::Response) -> &'static str {
    if resp.status() == reqwest::StatusCode::FORBIDDEN {
        " (this may be a rate limit; set GITHUB_TOKEN to raise it)"
    } else {
        ""
    }
}

fn user_agent() -> String {
    format!("ourshell-cli/{}", env!("CARGO_PKG_VERSION"))
}

/// The triple this binary was built for, baked in by `build.rs` from
/// Cargo's own `TARGET` -- guaranteed to match one of the targets
/// `.github/workflows/release.yml` builds for.
pub fn target_triple() -> &'static str {
    env!("OURSHELL_TARGET")
}

/// `zip` on Windows (matches `release.yml`'s `7z a *.zip`), `tar.gz` elsewhere.
fn archive_ext(target: &str) -> &'static str {
    if target.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    }
}

/// The exact archive and checksum-file names `release.yml` publishes for
/// `tag` + `target`, e.g. `ourshell-v0.6.0-x86_64-pc-windows-msvc.zip` and
/// `SHA256SUMS-x86_64-pc-windows-msvc.txt`.
pub fn asset_names(tag: &str, target: &str) -> (String, String) {
    let archive = format!("ourshell-{tag}-{target}.{}", archive_ext(target));
    let checksums = format!("SHA256SUMS-{target}.txt");
    (archive, checksums)
}

/// The current binary's version, from the version Cargo baked in at compile
/// time (kept equal to the workspace version by `scripts/check-version.mjs`
/// in CI, though this binary links it directly rather than trusting that).
pub fn current_version() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .expect("CARGO_PKG_VERSION is not valid semver")
}

/// Parses a release tag like `v0.6.0` (or `0.6.0`) into a [`semver::Version`].
pub fn parse_tag_version(tag: &str) -> Result<semver::Version> {
    let stripped = tag.strip_prefix('v').unwrap_or(tag);
    semver::Version::parse(stripped)
        .with_context(|| format!("release tag {tag:?} is not valid semver"))
}

/// True if `latest` is a newer release than `current`. Prerelease-aware via
/// `semver`'s own `Ord` (e.g. `1.0.0-alpha` < `1.0.0`), so a prerelease build
/// never reports a stable release of the same numbers as "no update" or
/// vice versa incorrectly.
pub fn is_newer(current: &semver::Version, latest: &semver::Version) -> bool {
    latest > current
}

/// Finds the asset named exactly `name` in the release, validating its URL
/// is one we're willing to download from before returning it.
fn find_asset<'a>(release: &'a ReleaseInfo, name: &str) -> Result<&'a GithubAsset> {
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == name)
        .ok_or_else(|| anyhow!("release {} has no asset named {name:?}", release.tag))?;
    validate_asset_url(&asset.browser_download_url)?;
    Ok(asset)
}

/// Picks the platform archive + its checksum file for `target` out of
/// `release`'s assets, matching the `ourshell-<tag>-<target>.<ext>` /
/// `SHA256SUMS-<target>.txt` names `release.yml` publishes.
pub fn pick_assets<'a>(release: &'a ReleaseInfo, target: &str) -> Result<(&'a str, &'a str)> {
    let (archive_name, checksums_name) = asset_names(&release.tag, target);
    let archive = find_asset(release, &archive_name)?;
    let checksums = find_asset(release, &checksums_name)?;
    Ok((
        &archive.browser_download_url,
        &checksums.browser_download_url,
    ))
}

/// Rejects any URL not under one of [`ALLOWED_URL_PREFIXES`].
fn validate_asset_url(url: &str) -> Result<()> {
    if ALLOWED_URL_PREFIXES.iter().any(|p| url.starts_with(p)) {
        Ok(())
    } else {
        bail!("refusing to download from untrusted URL: {url}")
    }
}

/// Downloads `url` to `dest`, validating both the requested URL and the
/// final (post-redirect) URL against [`ALLOWED_URL_PREFIXES`]. `reqwest`
/// follows redirects itself, so `response.url()` after `.get(url).send()` is
/// the URL actually fetched from.
pub async fn download_to(client: &reqwest::Client, url: &str, dest: &Path) -> Result<()> {
    validate_asset_url(url)?;
    let resp = client
        .get(url)
        .header("User-Agent", user_agent())
        .send()
        .await
        .with_context(|| format!("downloading {url}"))?;
    validate_asset_url(resp.url().as_str())
        .with_context(|| format!("redirect target for {url} was not a trusted host"))?;
    if !resp.status().is_success() {
        bail!("downloading {url} failed: HTTP {}", resp.status());
    }
    let bytes = resp.bytes().await.context("reading download body")?;
    std::fs::write(dest, &bytes).with_context(|| format!("writing {}", dest.display()))?;
    Ok(())
}

/// Parses a `sha256sum`-format checksums file (`<hex>␠␠<filename>` or
/// `<hex> *<filename>`, one per line) into a map from filename to lowercase
/// hex digest. Ignores blank lines and anything it can't parse, matching
/// `sha256sum`'s own tolerant `-c` behavior.
pub fn parse_checksums(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let Some(hex) = parts.next() else { continue };
        let Some(rest) = parts.next() else { continue };
        let name = rest.trim_start().trim_start_matches('*').trim();
        if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            map.insert(name.to_string(), hex.to_ascii_lowercase());
        }
    }
    map
}

/// Computes the lowercase hex SHA-256 of a file.
pub fn sha256_hex(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Verifies `archive_path` (whose file name is `archive_name`) matches the
/// digest recorded for it in `checksums_text`. Refuses (returns `Err`) on
/// any mismatch or missing entry; never partially trusts a file it can't
/// fully verify.
pub fn verify_checksum(
    archive_path: &Path,
    archive_name: &str,
    checksums_text: &str,
) -> Result<()> {
    let checksums = parse_checksums(checksums_text);
    let expected = checksums
        .get(archive_name)
        .ok_or_else(|| anyhow!("SHA256SUMS does not list {archive_name:?}"))?;
    let actual = sha256_hex(archive_path)?;
    if &actual != expected {
        bail!(
            "checksum mismatch for {archive_name}: expected {expected}, got {actual} -- refusing to install"
        );
    }
    Ok(())
}

/// The binaries extracted from a release archive.
pub struct ExtractedBinaries {
    pub ourshell: PathBuf,
    pub ourshell_mcp: Option<PathBuf>,
}

/// Extracts `ourshell(.exe)` and `ourshell-mcp(.exe)` from `archive_path`
/// (a `.zip` or `.tar.gz` produced by `release.yml`, whose entries are all
/// under a single `ourshell-<tag>-<target>/` directory) into `dest_dir`.
pub fn extract_archive(
    archive_path: &Path,
    dest_dir: &Path,
    target: &str,
) -> Result<ExtractedBinaries> {
    std::fs::create_dir_all(dest_dir)?;
    if archive_ext(target) == "zip" {
        extract_zip(archive_path, dest_dir)
    } else {
        extract_tar_gz(archive_path, dest_dir)
    }
}

fn binary_names(target: &str) -> (&'static str, &'static str) {
    if target.contains("windows") {
        ("ourshell.exe", "ourshell-mcp.exe")
    } else {
        ("ourshell", "ourshell-mcp")
    }
}

fn extract_zip(archive_path: &Path, dest_dir: &Path) -> Result<ExtractedBinaries> {
    let file = std::fs::File::open(archive_path)?;
    let mut zip = zip::ZipArchive::new(file).context("opening zip archive")?;
    let mut ourshell = None;
    let mut ourshell_mcp = None;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(file_name) = entry
            .enclosed_name()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        else {
            continue;
        };
        if file_name == "ourshell" || file_name == "ourshell.exe" {
            let dest = dest_dir.join(&file_name);
            let mut out = std::fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut out)?;
            ourshell = Some(dest);
        } else if file_name == "ourshell-mcp" || file_name == "ourshell-mcp.exe" {
            let dest = dest_dir.join(&file_name);
            let mut out = std::fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut out)?;
            ourshell_mcp = Some(dest);
        }
    }
    let ourshell = ourshell.ok_or_else(|| anyhow!("archive did not contain an ourshell binary"))?;
    Ok(ExtractedBinaries {
        ourshell,
        ourshell_mcp,
    })
}

fn extract_tar_gz(archive_path: &Path, dest_dir: &Path) -> Result<ExtractedBinaries> {
    let file = std::fs::File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut ourshell = None;
    let mut ourshell_mcp = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(file_name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        if file_name == "ourshell" {
            let dest = dest_dir.join(&file_name);
            let mut out = std::fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut out)?;
            make_executable(&dest)?;
            ourshell = Some(dest);
        } else if file_name == "ourshell-mcp" {
            let dest = dest_dir.join(&file_name);
            let mut out = std::fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut out)?;
            make_executable(&dest)?;
            ourshell_mcp = Some(dest);
        }
    }
    let ourshell = ourshell.ok_or_else(|| anyhow!("archive did not contain an ourshell binary"))?;
    Ok(ExtractedBinaries {
        ourshell,
        ourshell_mcp,
    })
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Stops a running agent, if any, so its executable isn't locked (matters on
/// Windows, where a running .exe can't be overwritten). Mirrors
/// `VaultCmd::StopAgent` in `commands/mod.rs`; best-effort, since "no agent
/// running" is the common case and not an error.
async fn stop_running_agent() {
    if let Ok((c, _)) = AgentClient::connect_existing(hello(ClientKind::Cli)).await {
        let _ = c
            .call::<_, serde_json::Value>(method::SHUTDOWN, &json!({}))
            .await;
        // Give the OS a moment to actually release the file handle on Windows.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

fn build_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building HTTP client")
}

/// `ourshell update --check`: prints current vs. latest and returns whether
/// an update is available (the caller turns that into exit code
/// [`EXIT_UPDATE_AVAILABLE`]).
pub async fn run_check(json_out: bool) -> Result<bool> {
    let client = build_client()?;
    let current = current_version();
    let release = fetch_latest_release(&client).await?;

    let Some(release) = release else {
        if json_out {
            println!(
                "{}",
                json!({ "current": current.to_string(), "latest": null, "update_available": false })
            );
        } else {
            println!("Current version: {current}");
            println!("No releases have been published yet.");
        }
        return Ok(false);
    };

    let latest = parse_tag_version(&release.tag)?;
    let available = is_newer(&current, &latest);

    if json_out {
        println!(
            "{}",
            json!({
                "current": current.to_string(),
                "latest": latest.to_string(),
                "update_available": available,
                "html_url": release.html_url,
            })
        );
    } else {
        println!("Current version: {current}");
        println!("Latest version:  {latest}");
        if available {
            println!("\nAn update is available. Run `ourshell update` to install it.");
        } else {
            println!("\nYou're on the latest version.");
        }
    }
    Ok(available)
}

/// `ourshell update`: downloads, verifies and installs the latest release
/// for this platform, replacing the running `ourshell` (and `ourshell-mcp`,
/// if found next to it).
pub async fn run_update(yes: bool) -> Result<()> {
    let client = build_client()?;
    let current = current_version();
    let release = fetch_latest_release(&client)
        .await?
        .ok_or_else(|| anyhow!("no releases have been published yet"))?;
    let latest = parse_tag_version(&release.tag)?;

    if !is_newer(&current, &latest) {
        println!("Already on the latest version ({current}).");
        return Ok(());
    }

    println!("Current version: {current}");
    println!("Latest version:  {latest}");
    if !release.notes.trim().is_empty() {
        println!(
            "\nRelease notes ({}):\n{}",
            release.html_url,
            release.notes.trim()
        );
    }

    if !yes {
        print!("\nInstall ourshell {latest}? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let target = target_triple();
    let (archive_name, checksums_name) = asset_names(&release.tag, target);
    let (archive_url, checksums_url) = pick_assets(&release, target)?;
    let archive_url = archive_url.to_string();
    let checksums_url = checksums_url.to_string();

    let tmp = tempfile::tempdir().context("creating a temp dir")?;
    let archive_path = tmp.path().join(&archive_name);
    let checksums_path = tmp.path().join(&checksums_name);

    println!("Downloading {archive_name}...");
    download_to(&client, &archive_url, &archive_path).await?;
    download_to(&client, &checksums_url, &checksums_path).await?;

    println!("Verifying checksum...");
    let checksums_text = std::fs::read_to_string(&checksums_path)?;
    verify_checksum(&archive_path, &archive_name, &checksums_text)?;

    println!("Extracting...");
    let extract_dir = tmp.path().join("extracted");
    let extracted = extract_archive(&archive_path, &extract_dir, target)?;

    println!("Stopping the running agent (if any)...");
    stop_running_agent().await;

    let current_exe = std::env::current_exe().context("locating the running executable")?;
    println!("Replacing {}...", current_exe.display());
    self_replace::self_replace(&extracted.ourshell)
        .with_context(|| format!("replacing {}", current_exe.display()))?;

    if let Some(new_mcp) = &extracted.ourshell_mcp {
        let (_, mcp_name) = binary_names(target);
        let mcp_path = current_exe.with_file_name(mcp_name);
        if mcp_path.exists() {
            match replace_sibling_binary(new_mcp, &mcp_path) {
                Ok(()) => println!("Replaced {}.", mcp_path.display()),
                Err(e) => eprintln!(
                    "warning: could not replace {} ({e}); it may be running. \
                     Close any AI client using it and run `ourshell update` again, \
                     or copy it manually from {}.",
                    mcp_path.display(),
                    new_mcp.display()
                ),
            }
        }
    }

    println!("\nUpdated to ourshell {latest}.");
    Ok(())
}

/// Replaces a binary that isn't the currently running process (unlike
/// `ourshell` itself, `ourshell-mcp` is a short-lived per-connection process,
/// so a plain copy generally works; it can still fail on Windows if an AI
/// client happens to have it open, which the caller reports as a warning).
fn replace_sibling_binary(new_path: &Path, dest: &Path) -> Result<()> {
    std::fs::copy(new_path, dest)?;
    #[cfg(unix)]
    make_executable(dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn semver_compares_plain_versions() {
        let current = semver::Version::parse("0.6.0").unwrap();
        let older = semver::Version::parse("0.5.9").unwrap();
        let newer = semver::Version::parse("0.6.1").unwrap();
        assert!(!is_newer(&current, &older));
        assert!(is_newer(&current, &newer));
        assert!(!is_newer(&current, &current));
    }

    #[test]
    fn semver_is_prerelease_aware() {
        let stable = semver::Version::parse("1.0.0").unwrap();
        let rc = semver::Version::parse("1.0.0-rc.1").unwrap();
        // A prerelease sorts *before* the final release of the same numbers.
        assert!(is_newer(&rc, &stable));
        assert!(!is_newer(&stable, &rc));
    }

    #[test]
    fn parse_tag_version_strips_v_prefix() {
        assert_eq!(parse_tag_version("v1.2.3").unwrap().to_string(), "1.2.3");
        assert_eq!(parse_tag_version("1.2.3").unwrap().to_string(), "1.2.3");
        assert!(parse_tag_version("not-a-version").is_err());
    }

    #[test]
    fn asset_names_pick_zip_for_windows_and_targz_elsewhere() {
        let (archive, checksums) = asset_names("v0.6.0", "x86_64-pc-windows-msvc");
        assert_eq!(archive, "ourshell-v0.6.0-x86_64-pc-windows-msvc.zip");
        assert_eq!(checksums, "SHA256SUMS-x86_64-pc-windows-msvc.txt");

        let (archive, checksums) = asset_names("v0.6.0", "x86_64-unknown-linux-gnu");
        assert_eq!(archive, "ourshell-v0.6.0-x86_64-unknown-linux-gnu.tar.gz");
        assert_eq!(checksums, "SHA256SUMS-x86_64-unknown-linux-gnu.txt");
    }

    #[test]
    fn pick_assets_matches_the_right_target_and_rejects_untrusted_urls() {
        let release = ReleaseInfo {
            tag: "v0.6.0".to_string(),
            html_url: "https://github.com/NakharinA/OurShell/releases/tag/v0.6.0".to_string(),
            notes: String::new(),
            assets: vec![
                GithubAsset {
                    name: "ourshell-v0.6.0-x86_64-unknown-linux-gnu.tar.gz".to_string(),
                    browser_download_url:
                        "https://github.com/NakharinA/OurShell/releases/download/v0.6.0/ourshell-v0.6.0-x86_64-unknown-linux-gnu.tar.gz"
                            .to_string(),
                },
                GithubAsset {
                    name: "SHA256SUMS-x86_64-unknown-linux-gnu.txt".to_string(),
                    browser_download_url:
                        "https://github.com/NakharinA/OurShell/releases/download/v0.6.0/SHA256SUMS-x86_64-unknown-linux-gnu.txt"
                            .to_string(),
                },
                GithubAsset {
                    name: "ourshell-v0.6.0-aarch64-apple-darwin.tar.gz".to_string(),
                    browser_download_url: "https://evil.example.com/ourshell.tar.gz".to_string(),
                },
            ],
        };

        let (archive_url, checksums_url) =
            pick_assets(&release, "x86_64-unknown-linux-gnu").unwrap();
        assert!(archive_url.starts_with("https://github.com/NakharinA/OurShell/releases/download/"));
        assert!(
            checksums_url.starts_with("https://github.com/NakharinA/OurShell/releases/download/")
        );

        // The darwin asset's URL is untrusted, so picking that target must fail
        // even though the name matches -- `find_asset` validates the URL.
        let err = pick_assets(&release, "aarch64-apple-darwin").unwrap_err();
        assert!(err.to_string().contains("untrusted"));
    }

    #[test]
    fn validate_asset_url_allows_only_known_hosts() {
        assert!(
            validate_asset_url("https://github.com/NakharinA/OurShell/releases/download/v1/x")
                .is_ok()
        );
        assert!(validate_asset_url("https://objects.githubusercontent.com/abc").is_ok());
        assert!(validate_asset_url("https://release-assets.githubusercontent.com/abc").is_ok());
        assert!(validate_asset_url("https://evil.example.com/x").is_err());
        assert!(
            validate_asset_url("http://github.com/NakharinA/OurShell/releases/download/v1/x")
                .is_err()
        );
    }

    #[test]
    fn parse_checksums_reads_sha256sum_format() {
        let text = "\
deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef  ourshell-v0.6.0-x86_64-unknown-linux-gnu.tar.gz
cafebabecafebabecafebabecafebabecafebabecafebabecafebabecafebabe *SHA256SUMS-other.txt
";
        let map = parse_checksums(text);
        assert_eq!(
            map.get("ourshell-v0.6.0-x86_64-unknown-linux-gnu.tar.gz")
                .unwrap(),
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
        );
        assert_eq!(
            map.get("SHA256SUMS-other.txt").unwrap(),
            "cafebabecafebabecafebabecafebabecafebabecafebabecafebabecafebabe"
        );
    }

    #[test]
    fn verify_checksum_accepts_matching_and_refuses_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir
            .path()
            .join("ourshell-v0.6.0-x86_64-unknown-linux-gnu.tar.gz");
        std::fs::write(&archive_path, b"not a real archive, just test bytes").unwrap();
        let actual = sha256_hex(&archive_path).unwrap();

        let good = format!("{actual}  ourshell-v0.6.0-x86_64-unknown-linux-gnu.tar.gz\n");
        verify_checksum(
            &archive_path,
            "ourshell-v0.6.0-x86_64-unknown-linux-gnu.tar.gz",
            &good,
        )
        .unwrap();

        let bad = "0000000000000000000000000000000000000000000000000000000000000000  ourshell-v0.6.0-x86_64-unknown-linux-gnu.tar.gz\n";
        let err = verify_checksum(
            &archive_path,
            "ourshell-v0.6.0-x86_64-unknown-linux-gnu.tar.gz",
            bad,
        )
        .unwrap_err();
        assert!(err.to_string().contains("mismatch"));

        let missing = verify_checksum(&archive_path, "not-listed.tar.gz", &good).unwrap_err();
        assert!(missing.to_string().contains("does not list"));
    }

    #[test]
    fn extract_tar_gz_finds_both_binaries() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir
            .path()
            .join("ourshell-v0.6.0-x86_64-unknown-linux-gnu.tar.gz");

        let file = std::fs::File::create(&archive_path).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(enc);
        add_tar_file(
            &mut builder,
            "ourshell-v0.6.0-x86_64-unknown-linux-gnu/ourshell",
            b"fake cli",
        );
        add_tar_file(
            &mut builder,
            "ourshell-v0.6.0-x86_64-unknown-linux-gnu/ourshell-mcp",
            b"fake mcp",
        );
        add_tar_file(
            &mut builder,
            "ourshell-v0.6.0-x86_64-unknown-linux-gnu/README.md",
            b"not a binary",
        );
        builder.into_inner().unwrap().finish().unwrap();

        let dest_dir = dir.path().join("out");
        let extracted =
            extract_archive(&archive_path, &dest_dir, "x86_64-unknown-linux-gnu").unwrap();
        assert_eq!(std::fs::read(&extracted.ourshell).unwrap(), b"fake cli");
        assert_eq!(
            std::fs::read(extracted.ourshell_mcp.unwrap()).unwrap(),
            b"fake mcp"
        );
    }

    fn add_tar_file<W: std::io::Write>(builder: &mut tar::Builder<W>, path: &str, contents: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_path(path).unwrap();
        header.set_size(contents.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append(&header, contents).unwrap();
    }

    #[test]
    fn extract_zip_finds_both_binaries() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir
            .path()
            .join("ourshell-v0.6.0-x86_64-pc-windows-msvc.zip");

        let file = std::fs::File::create(&archive_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        zip.start_file("ourshell-v0.6.0-x86_64-pc-windows-msvc/ourshell.exe", opts)
            .unwrap();
        zip.write_all(b"fake cli").unwrap();
        zip.start_file(
            "ourshell-v0.6.0-x86_64-pc-windows-msvc/ourshell-mcp.exe",
            opts,
        )
        .unwrap();
        zip.write_all(b"fake mcp").unwrap();
        zip.finish().unwrap();

        let dest_dir = dir.path().join("out");
        let extracted =
            extract_archive(&archive_path, &dest_dir, "x86_64-pc-windows-msvc").unwrap();
        assert_eq!(std::fs::read(&extracted.ourshell).unwrap(), b"fake cli");
        assert_eq!(
            std::fs::read(extracted.ourshell_mcp.unwrap()).unwrap(),
            b"fake mcp"
        );
    }
}
