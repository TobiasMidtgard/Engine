//! # fetch-assets
//!
//! Large art (scanned textures, HDRIs, models, recorded sounds) never goes into git. Each area
//! lists the files it needs in `assets/manifests/<area>.toml`; this tool downloads them into
//! `assets/fetched/` (gitignored), checks every file against its pinned SHA-256, and keeps a
//! download cache shared by every checkout on the machine.
//!
//! ```text
//! cargo run -p fetch-assets                  fetch everything listed
//! cargo run -p fetch-assets -- showcase      only assets/manifests/showcase.toml
//! cargo run -p fetch-assets -- check         validate the manifests (offline)
//! cargo run -p fetch-assets -- mirror        CI only: copy listed files to the mirror, print pins
//! ```
//!
//! A file comes from, in order: the local cache, the GitHub release mirror (`asset-mirror` on
//! the engine repository, filled by the Assets workflow, so it works where the original host
//! is blocked), then its original URL.
//!
//! A manifest entry:
//!
//! ```toml
//! [[file]]
//! path = "showcase/rock_face/rock_face_diff_2k.jpg"   # under assets/fetched/
//! url = "https://dl.polyhaven.org/file/ph-assets/Textures/jpg/2k/rock_face/rock_face_diff_2k.jpg"
//! sha256 = ""                    # empty until pinned: the Assets workflow prints the value
//! license = "CC0-1.0"
//! source = "https://polyhaven.com/a/rock_face"
//! author = "Rob Tuytel"          # required for CC-BY
//! ```

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Largest file that may be committed to git. Anything bigger goes in a manifest.
pub const MAX_COMMITTED_BYTES: u64 = 1024 * 1024;

/// Tracked files allowed past [`MAX_COMMITTED_BYTES`].
pub const LARGE_FILE_ALLOWLIST: &[&str] = &["Cargo.lock"];

/// Licenses a fetched file may have. Anything non-commercial, no-derivatives or unknown is out.
pub const LICENSES: &[&str] = &[
    "CC0-1.0",
    "CC-BY-4.0",
    "CC-BY-3.0",
    "OFL-1.1",
    "MIT",
    "Apache-2.0",
    "MIT OR Apache-2.0",
];

/// Release on the engine repository that holds a copy of every fetched file, named by SHA-256.
pub const MIRROR_TAG: &str = "asset-mirror";

/// Repository of the mirror (`owner/name`); `ENGINE_ASSET_MIRROR` overrides it.
pub const DEFAULT_MIRROR_REPO: &str = "TobiasMidtgard/Engine";

/// One file to fetch.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Where it lands, relative to `assets/fetched/` (forward slashes). Load it with the asset
    /// key `fetched/<path>`.
    pub path: String,
    /// Direct download URL (https).
    pub url: String,
    /// Lowercase hex SHA-256 of the file. Empty until pinned.
    #[serde(default)]
    pub sha256: String,
    /// SPDX id, one of [`LICENSES`].
    pub license: String,
    /// Page that shows the asset and its license (https).
    pub source: String,
    /// Creator; required for attribution licenses (CC-BY).
    #[serde(default)]
    pub author: Option<String>,
}

impl Entry {
    /// True once `sha256` is filled in.
    pub fn is_pinned(&self) -> bool {
        !self.sha256.is_empty()
    }

    /// The asset key engine code loads it with (`fetched/<path>`).
    pub fn asset_key(&self) -> String {
        format!("fetched/{}", self.path)
    }
}

/// One `assets/manifests/<name>.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// File stem, usually the owning area (`showcase`, `foliage`, …).
    pub name: String,
    pub file: PathBuf,
    pub files: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    #[serde(default)]
    file: Vec<Entry>,
}

/// The repository root (where `Cargo.toml`, `assets/` and `.git` live).
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")))
}

/// `assets/manifests` under `root`.
pub fn manifest_dir(root: &Path) -> PathBuf {
    root.join("assets").join("manifests")
}

/// `assets/fetched` under `root`, where files land.
pub fn fetched_dir(root: &Path) -> PathBuf {
    root.join("assets").join("fetched")
}

/// Parse one manifest from text.
pub fn parse_manifest(name: &str, file: &Path, text: &str) -> Result<Manifest, String> {
    let parsed: ManifestFile =
        toml::from_str(text).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(Manifest {
        name: name.to_string(),
        file: file.to_path_buf(),
        files: parsed.file,
    })
}

/// Every `*.toml` in `assets/manifests`, sorted by name. No folder means no manifests.
pub fn load_manifests(root: &Path) -> Result<Vec<Manifest>, String> {
    let dir = manifest_dir(root);
    let Ok(read) = fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut manifests = Vec::new();
    for item in read {
        let path = item.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        manifests.push(parse_manifest(&name, &path, &text)?);
    }
    manifests.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(manifests)
}

/// Everything wrong with the manifests, as sentences that say what to fix. Unpinned entries
/// are reported only when `require_pins` is set (CI requires pins before a merge; fetching
/// works without them).
pub fn validate(manifests: &[Manifest], require_pins: bool) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen: HashMap<&str, &str> = HashMap::new();
    for m in manifests {
        if m.name.is_empty()
            || !m
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            problems.push(format!(
                "{}: name manifests with lowercase letters, digits, '-' or '_'",
                m.file.display()
            ));
        }
        for e in &m.files {
            let at = format!("{} ({})", m.name, e.path);
            if let Err(why) = check_path(&e.path) {
                problems.push(format!("{at}: {why}"));
            }
            if let Some(other) = seen.insert(&e.path, &m.name) {
                problems.push(format!(
                    "{at}: the same path is also listed in {other}; give one of them another path"
                ));
            }
            if !e.url.starts_with("https://") {
                problems.push(format!("{at}: url must be an https download link"));
            }
            if !e.source.starts_with("https://") {
                problems.push(format!(
                    "{at}: source must be the https page that shows the asset and its license"
                ));
            }
            if e.is_pinned() && !is_sha256(&e.sha256) {
                problems.push(format!(
                    "{at}: sha256 must be 64 lowercase hex digits (or empty until pinned)"
                ));
            }
            if require_pins && !e.is_pinned() {
                problems.push(format!(
                    "{at}: not pinned yet; copy the sha256 the Assets workflow (or `cargo run -p fetch-assets`) printed"
                ));
            }
            if !LICENSES.contains(&e.license.as_str()) {
                problems.push(format!(
                    "{at}: license {:?} is not allowed; use one of {}",
                    e.license,
                    LICENSES.join(", ")
                ));
            }
            if e.license.starts_with("CC-BY")
                && e.author.as_deref().is_none_or(|a| a.trim().is_empty())
            {
                problems.push(format!("{at}: {} needs an author to credit", e.license));
            }
        }
    }
    problems
}

/// A manifest path must stay inside `assets/fetched/` and work on every OS.
pub fn check_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("path is empty".into());
    }
    if path.contains('\\') || path.contains(':') {
        return Err("use forward slashes and no ':' in path".into());
    }
    let p = Path::new(path);
    if !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err("path must be relative to assets/fetched/ without '..' or '.'".into());
    }
    if p.extension().is_none() {
        return Err("path needs a file extension".into());
    }
    Ok(())
}

/// 64 lowercase hex digits.
pub fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Lowercase hex SHA-256 of a file.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Download cache shared by every checkout: `ENGINE_ASSET_CACHE`, else the user cache folder
/// (`%LOCALAPPDATA%\engine-assets`, `~/Library/Caches/engine-assets`,
/// `$XDG_CACHE_HOME/engine-assets` or `~/.cache/engine-assets`).
pub fn cache_dir() -> PathBuf {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = var("ENGINE_ASSET_CACHE") {
        return dir;
    }
    let base = if cfg!(windows) {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Caches"))
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|h| h.join(".cache")))
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join("engine-assets")
}

/// Tracked files bigger than [`MAX_COMMITTED_BYTES`] (path, bytes), outside the allowlist.
/// `None` when git can't list the files (a source archive without `.git`).
pub fn oversized_tracked_files(root: &Path) -> Option<Vec<(String, u64)>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let mut big = Vec::new();
    for name in out.stdout.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        let name = String::from_utf8_lossy(name).into_owned();
        if LARGE_FILE_ALLOWLIST.contains(&name.as_str()) {
            continue;
        }
        // Deleted but not yet committed files have no metadata; they are not a problem.
        if let Ok(meta) = fs::metadata(root.join(&name))
            && meta.len() > MAX_COMMITTED_BYTES
        {
            big.push((name, meta.len()));
        }
    }
    Some(big)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
[[file]]
path = "showcase/rock_face/rock_face_diff_2k.jpg"
url = "https://dl.polyhaven.org/file/ph-assets/Textures/jpg/2k/rock_face/rock_face_diff_2k.jpg"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
license = "CC0-1.0"
source = "https://polyhaven.com/a/rock_face"

[[file]]
path = "showcase/helmet/helmet.glb"
url = "https://example.com/helmet.glb"
license = "CC-BY-4.0"
source = "https://example.com/helmet"
author = "Someone"
"#;

    fn manifest(name: &str, text: &str) -> Manifest {
        parse_manifest(name, Path::new("test.toml"), text).unwrap()
    }

    #[test]
    fn a_good_manifest_passes_and_unpinned_files_wait_for_their_pin() {
        let m = manifest("showcase", GOOD);
        assert_eq!(m.files.len(), 2);
        assert_eq!(
            m.files[0].asset_key(),
            "fetched/showcase/rock_face/rock_face_diff_2k.jpg"
        );
        assert!(validate(std::slice::from_ref(&m), false).is_empty());
        let problems = validate(&[m], true);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("not pinned"), "{problems:?}");
    }

    #[test]
    fn bad_entries_are_explained() {
        let bad = r#"
[[file]]
path = "../outside.png"
url = "http://example.com/a.png"
sha256 = "ABC"
license = "CC-BY-NC-4.0"
source = "example.com"

[[file]]
path = "credit/me.png"
url = "https://example.com/b.png"
license = "CC-BY-4.0"
source = "https://example.com/b"
"#;
        let problems = validate(&[manifest("Bad Name", bad)], false);
        for expected in [
            "name manifests",
            "without '..'",
            "https download link",
            "https page",
            "64 lowercase hex",
            "is not allowed",
            "needs an author",
        ] {
            assert!(
                problems.iter().any(|p| p.contains(expected)),
                "missing {expected:?} in {problems:#?}"
            );
        }
    }

    #[test]
    fn two_manifests_cannot_claim_one_path() {
        let problems = validate(&[manifest("a", GOOD), manifest("b", GOOD)], false);
        assert_eq!(
            problems
                .iter()
                .filter(|p| p.contains("also listed in a"))
                .count(),
            2,
            "{problems:#?}"
        );
    }

    #[test]
    fn typos_in_field_names_are_errors() {
        let typo = r#"
[[file]]
path = "a.png"
url = "https://example.com/a.png"
licence = "CC0-1.0"
source = "https://example.com/a"
"#;
        assert!(parse_manifest("a", Path::new("a.toml"), typo).is_err());
    }

    #[test]
    fn paths_stay_inside_the_fetched_folder() {
        assert!(check_path("a/b/c.png").is_ok());
        for bad in [
            "",
            "/abs.png",
            "a/../b.png",
            "./a.png",
            "a\\b.png",
            "c:x.png",
            "noext",
        ] {
            assert!(check_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn files_hash_to_their_sha256() {
        let dir = std::env::temp_dir().join(format!("fetch-assets-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("abc.txt");
        fs::write(&file, b"abc").unwrap();
        assert_eq!(
            sha256_file(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
