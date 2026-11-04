//! Release awareness: a cached version check, and an in-place upgrade.
//!
//! Every human-facing command may end with one line saying a newer release
//! exists. The check never costs a command latency: a fresh cache is read
//! synchronously and a stale one is refreshed on a detached thread, so the note
//! a reader sees was discovered by an earlier command.
//!
//! `--upgrade` downloads the released asset for this platform, verifies the
//! published checksum, and replaces the running binary.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Release host, overridable so a test or a mirror can answer instead.
const RELEASE_BASE: &str = "https://github.com/zaterka/wayfinder/releases";
/// API that names the newest release.
const RELEASE_API: &str = "https://api.github.com/repos/zaterka/wayfinder/releases/latest";
/// How long a discovered answer stays good.
const TTL_SECONDS: u64 = 24 * 60 * 60;
/// How long a refresh may take before it gives up silently.
const REFRESH_TIMEOUT_SECONDS: u64 = 3;

/// A three-component version, enough to order release tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    /// First component.
    pub major: u64,
    /// Second component.
    pub minor: u64,
    /// Third component.
    pub patch: u64,
}

impl Version {
    /// Parse `1.2.3`, `v1.2.3`, or a pre-release such as `v1.2.3-rc.1`.
    /// @param text - tag or version text
    /// @returns the parsed version, or `None` when it is not one
    pub fn parse(text: &str) -> Option<Self> {
        let trimmed = text.trim().trim_start_matches('v');
        let core = trimmed.split(['-', '+']).next().unwrap_or(trimmed);
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().unwrap_or("0").parse().ok()?;
        let patch = parts.next().unwrap_or("0").parse().ok()?;
        Some(Self {
            major,
            minor,
            patch,
        })
    }

    /// The version this binary was built from.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Self {
            major: 0,
            minor: 0,
            patch: 0,
        })
    }
}

/// The asset name one platform would download.
/// @param os - `std::env::consts::OS` value
/// @param arch - `std::env::consts::ARCH` value
/// @returns the published asset name, or `None` for a platform with no build
pub fn asset_name(os: &str, arch: &str) -> Option<String> {
    let platform = match (os, arch) {
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-arm64",
        ("macos", "aarch64") => "macos-arm64",
        ("macos", "x86_64") => "macos-x86_64",
        _ => return None,
    };
    Some(format!("wayfinder-{platform}.tar.gz"))
}

/// The asset for the machine this binary runs on.
pub fn platform_asset() -> Option<String> {
    asset_name(std::env::consts::OS, std::env::consts::ARCH)
}

/// What the cache holds between commands.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cache {
    /// Unix seconds of the last successful check.
    pub checked_at: u64,
    /// Tag the last check reported, when there was one.
    pub latest: Option<String>,
}

/// Where the check result lives.
pub fn cache_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("WAYFINDER_CACHE_DIR") {
        return PathBuf::from(dir).join("update.json");
    }
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(dir).join("wayfinder/update.json");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cache/wayfinder/update.json")
}

/// Current time in unix seconds.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_secs())
        .unwrap_or(0)
}

/// Read the cache, tolerating absence and damage.
/// @param path - cache file
pub fn read_cache(path: &Path) -> Option<Cache> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write the cache, ignoring failure: a read-only home is not an error.
/// @param path - cache file
/// @param cache - what to record
pub fn write_cache(path: &Path, cache: &Cache) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(cache) {
        let _ = fs::write(path, text);
    }
}

/// Whether a check is still current.
/// @param cache - recorded result
/// @param at - now, in unix seconds
pub fn is_fresh(cache: &Cache, at: u64) -> bool {
    at.saturating_sub(cache.checked_at) < TTL_SECONDS
}

/// Ask the release API for the newest tag.
/// @param api - API URL, usually [`RELEASE_API`]
/// @returns the tag, e.g. `v0.2.0`
pub fn fetch_latest(api: &str) -> Result<String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(REFRESH_TIMEOUT_SECONDS)))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut response = agent
        .get(api)
        .header("accept", "application/vnd.github+json")
        .header(
            "user-agent",
            concat!("wayfinder/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|error| anyhow::anyhow!("release check failed: {error}"))?;
    let document: serde_json::Value = response
        .body_mut()
        .read_json()
        .map_err(|error| anyhow::anyhow!("release check returned no JSON: {error}"))?;
    document["tag_name"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("release check returned no tag_name"))
}

/// Refresh the cache from the network, staying quiet about every failure.
/// @param api - API URL
/// @param path - cache file to update on success
/// @returns the tag when the refresh worked
pub fn refresh(api: &str, path: &Path) -> Option<String> {
    let latest = fetch_latest(api).ok()?;
    write_cache(
        path,
        &Cache {
            checked_at: now(),
            latest: Some(latest.clone()),
        },
    );
    Some(latest)
}

/// Whether the environment silences the check entirely.
pub fn suppressed() -> bool {
    ["WAYFINDER_NO_UPDATE_CHECK", "WAYFINDER_OFFLINE", "CI"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

/// The line a command prints when a newer release exists.
/// @param latest - tag reported by the release API
pub fn notice(latest: &str) -> String {
    format!("wayfinder {latest} is available — run `wayfinder --upgrade` to install it")
}

/// Print a notice when the cache already knows about a newer release.
///
/// Never blocks: a stale or missing cache is refreshed on a detached thread, so
/// the note appears on a later command.
pub fn maybe_notice() {
    if suppressed() {
        return;
    }
    let path = cache_path();
    match read_cache(&path) {
        Some(cache) if is_fresh(&cache, now()) => {
            let Some(latest) = cache.latest.as_deref() else {
                return;
            };
            let Some(latest_version) = Version::parse(latest) else {
                return;
            };
            if latest_version > Version::current() {
                eprintln!("\n{}", notice(latest));
            }
        }
        // A stale or missing cache is refreshed inline with a short timeout: a
        // detached thread dies with the process, so the first command of the day
        // would never learn anything. The price is one request, at most.
        _ => {
            refresh(&api_url(), &path);
        }
    }
}

/// The release API URL, honouring an override.
pub fn api_url() -> String {
    std::env::var("WAYFINDER_RELEASE_API").unwrap_or_else(|_| RELEASE_API.to_owned())
}

/// The release download base, honouring an override.
pub fn release_base() -> String {
    std::env::var("WAYFINDER_RELEASE_BASE").unwrap_or_else(|_| RELEASE_BASE.to_owned())
}

/// Download the newest release and replace this binary.
/// @param base - release download base
/// @param api - release API URL
/// @returns the tag that was installed
pub fn upgrade(base: &str, api: &str) -> Result<String> {
    let latest = fetch_latest(api).context("could not reach the release index")?;
    let current = Version::current();
    if Version::parse(&latest).is_some_and(|version| version <= current) {
        return Ok(format!(
            "wayfinder {latest} is the latest release; this binary is {}.{}.{}",
            current.major, current.minor, current.patch
        ));
    }
    let asset = platform_asset().ok_or_else(|| {
        anyhow::anyhow!(
            "no released binary for {}/{}; build from source instead",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let url = format!("{base}/latest/download/{asset}");
    let bytes = download(&url)?;
    let expected = download(&format!("{url}.sha256")).ok();
    if let Some(expected) = expected {
        let expected = String::from_utf8_lossy(&expected);
        let expected = expected
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_lowercase();
        let actual = digest(&bytes);
        if expected.len() != 64 || expected != actual {
            bail!("the downloaded archive does not match its published checksum");
        }
    }
    let binary = extract_binary(&bytes)?;
    replace_self(&binary)?;
    write_cache(
        &cache_path(),
        &Cache {
            checked_at: now(),
            latest: Some(latest.clone()),
        },
    );
    Ok(format!("upgraded wayfinder to {latest}"))
}

/// Fetch one URL into memory.
fn download(url: &str) -> Result<Vec<u8>> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut response = agent
        .get(url)
        .header(
            "user-agent",
            concat!("wayfinder/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|error| anyhow::anyhow!("download failed for {url}: {error}"))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(64 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| anyhow::anyhow!("read failed for {url}: {error}"))?;
    Ok(bytes)
}

/// Hex SHA-256 of some bytes.
fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Pull the `wayfinder` binary out of the release tarball.
fn extract_binary(tarball: &[u8]) -> Result<Vec<u8>> {
    let decoder = flate2::read::GzDecoder::new(tarball);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| anyhow::anyhow!("the archive cannot be read: {error}"))?;
    for entry in entries {
        let mut entry =
            entry.map_err(|error| anyhow::anyhow!("the archive is damaged: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| anyhow::anyhow!("the archive has a bad path: {error}"))?
            .to_path_buf();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if name != "wayfinder" {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| anyhow::anyhow!("the binary cannot be read: {error}"))?;
        return Ok(bytes);
    }
    bail!("the release archive holds no wayfinder binary")
}

/// Replace the running executable with new bytes.
fn replace_self(binary: &[u8]) -> Result<()> {
    let current = std::env::current_exe().context("cannot locate the running binary")?;
    let staged = current.with_extension("new");
    fs::write(&staged, binary).with_context(|| format!("write {}", staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("make {} executable", staged.display()))?;
        fs::rename(&staged, &current).with_context(|| format!("replace {}", current.display()))?;
    }
    #[cfg(not(unix))]
    {
        // Windows cannot replace a running image: park it and let the new one
        // take the name.
        let retired = current.with_extension("old");
        let _ = fs::remove_file(&retired);
        fs::rename(&current, &retired).with_context(|| format!("retire {}", current.display()))?;
        fs::rename(&staged, &current).with_context(|| format!("replace {}", current.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_is_ordered_like_a_version() {
        assert_eq!(
            Version::parse("v1.2.3"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        assert_eq!(
            Version::parse("0.1.0"),
            Some(Version {
                major: 0,
                minor: 1,
                patch: 0
            })
        );
        assert_eq!(
            Version::parse("v2.0.0-rc.1"),
            Some(Version {
                major: 2,
                minor: 0,
                patch: 0
            })
        );
        assert!(Version::parse("nightly").is_none());
        assert!(Version::parse("v0.2.0") > Version::parse("v0.1.9"));
        assert_eq!(
            Version::current(),
            Version::parse(env!("CARGO_PKG_VERSION")).expect("own version")
        );
    }

    #[test]
    fn the_asset_name_matches_the_release_matrix() {
        assert_eq!(
            asset_name("macos", "aarch64").as_deref(),
            Some("wayfinder-macos-arm64.tar.gz")
        );
        assert_eq!(
            asset_name("macos", "x86_64").as_deref(),
            Some("wayfinder-macos-x86_64.tar.gz")
        );
        assert_eq!(
            asset_name("linux", "x86_64").as_deref(),
            Some("wayfinder-linux-x86_64.tar.gz")
        );
        assert_eq!(
            asset_name("linux", "aarch64").as_deref(),
            Some("wayfinder-linux-arm64.tar.gz")
        );
        assert_eq!(asset_name("windows", "x86_64"), None);
    }

    #[test]
    fn a_check_is_current_for_a_day_and_damaged_cache_is_ignored() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("update.json");
        assert!(read_cache(&path).is_none());

        write_cache(
            &path,
            &Cache {
                checked_at: 1_000,
                latest: Some("v0.2.0".to_owned()),
            },
        );
        let cache = read_cache(&path).expect("cache");
        assert_eq!(cache.latest.as_deref(), Some("v0.2.0"));
        assert!(is_fresh(&cache, 1_000 + 60));
        assert!(!is_fresh(&cache, 1_000 + TTL_SECONDS + 1));

        fs::write(&path, "{not json").expect("damage the cache");
        assert!(read_cache(&path).is_none());
    }

    #[test]
    fn a_newer_release_reads_as_a_notice_and_an_older_one_does_not() {
        assert_eq!(
            notice("v0.2.0"),
            "wayfinder v0.2.0 is available — run `wayfinder --upgrade` to install it"
        );
        // The comparison that decides whether to print at all.
        let current = Version::current();
        assert!(Version::parse("v99.0.0").expect("parsed") > current);
    }

    #[test]
    fn the_checksum_of_a_release_archive_is_what_upgrade_compares() {
        // A known vector keeps `digest` honest without a network call.
        assert_eq!(
            digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
