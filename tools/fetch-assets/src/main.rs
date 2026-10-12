//! `cargo run -p fetch-assets [-- <manifest>... | check | mirror | publish]`. See the crate docs.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::UNIX_EPOCH;

use fetch_assets::*;
use serde::{Deserialize, Serialize};

const USAGE: &str = "\
fetch-assets: download the large art listed in assets/manifests/*.toml into assets/fetched/

  cargo run -p fetch-assets                   fetch everything listed
  cargo run -p fetch-assets -- <name>...      only assets/manifests/<name>.toml
  cargo run -p fetch-assets -- check          validate the manifests (offline, needs pins)
  cargo run -p fetch-assets -- mirror         CI only: copy files to the release mirror
  cargo run -p fetch-assets -- publish [<name>...]
                                              upload files made here (url = \"mirror\") from
                                              assets/fetched/ to the mirror and print their pins

Cache: ENGINE_ASSET_CACHE (default: the user cache folder). Mirror repository:
ENGINE_ASSET_MIRROR (default TobiasMidtgard/Engine). GitHub token: GITHUB_TOKEN, GH_TOKEN or
`gh auth token`.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = workspace_root();
    let result = match args.first().map(String::as_str) {
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some("check") => check(&root),
        Some("mirror") => mirror(&root),
        Some("publish") => publish(&root, &args[1..]),
        _ => fetch(&root, &args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("fetch-assets: {message}");
            ExitCode::FAILURE
        }
    }
}

fn check(root: &Path) -> Result<(), String> {
    let manifests = load_manifests(root)?;
    let problems = validate(&manifests, true);
    for p in &problems {
        eprintln!("  {p}");
    }
    let files: usize = manifests.iter().map(|m| m.files.len()).sum();
    if problems.is_empty() {
        println!("{} manifests, {files} files: all good", manifests.len());
        Ok(())
    } else {
        Err(format!("{} problems in the manifests", problems.len()))
    }
}

/// What was last placed in `assets/fetched/`, so unchanged files skip re-hashing.
#[derive(Default, Serialize, Deserialize)]
struct Placed {
    files: HashMap<String, PlacedFile>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct PlacedFile {
    sha256: String,
    len: u64,
    modified_ns: u128,
}

impl PlacedFile {
    fn of(path: &Path, sha256: &str) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        let modified_ns = meta
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(Self {
            sha256: sha256.to_string(),
            len: meta.len(),
            modified_ns,
        })
    }
}

/// The valid manifests, only those named in `only` when it isn't empty.
fn selected_manifests(root: &Path, only: &[String]) -> Result<Vec<Manifest>, String> {
    let mut manifests = load_manifests(root)?;
    let known: HashSet<&str> = manifests.iter().map(|m| m.name.as_str()).collect();
    if let Some(unknown) = only.iter().find(|n| !known.contains(n.as_str())) {
        return Err(format!(
            "no assets/manifests/{unknown}.toml (have: {})",
            known.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    let problems = validate(&manifests, false);
    if !problems.is_empty() {
        for p in &problems {
            eprintln!("  {p}");
        }
        return Err("fix the manifests first".into());
    }
    if !only.is_empty() {
        manifests.retain(|m| only.contains(&m.name));
    }
    Ok(manifests)
}

fn fetch(root: &Path, only: &[String]) -> Result<(), String> {
    let manifests = selected_manifests(root, only)?;

    let fetched = fetched_dir(root);
    let cache = cache_dir();
    fs::create_dir_all(&cache).map_err(|e| format!("can't create {}: {e}", cache.display()))?;
    let state_file = fetched.join(".placed.json");
    let mut placed: Placed = fs::read(&state_file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let mut mirror = Mirror::new();
    let (mut ready, mut downloaded, mut failed, mut pins) = (0, 0, Vec::new(), Vec::new());

    for m in &manifests {
        for e in &m.files {
            let target = fetched.join(&e.path);
            let result = if e.is_pinned() {
                place_pinned(e, &target, &cache, &mut mirror, &mut placed)
            } else if e.is_made_here() {
                if target.exists() {
                    pins.push(format!(
                        "{} ({}): made here; `cargo run -p fetch-assets -- publish` uploads it and prints the pin",
                        m.name, e.path
                    ));
                    Ok(false)
                } else {
                    Err("made here and not published yet: whoever has the file puts it in assets/fetched/ and runs `cargo run -p fetch-assets -- publish`".into())
                }
            } else {
                place_unpinned(e, &target, &cache).map(|sha| {
                    pins.push(format!("{} ({}): sha256 = \"{sha}\"", m.name, e.path));
                    true
                })
            };
            match result {
                Ok(true) => downloaded += 1,
                Ok(false) => ready += 1,
                Err(why) => failed.push(format!("{} ({}): {why}", m.name, e.path)),
            }
        }
    }

    if let Ok(json) = serde_json::to_vec_pretty(&placed) {
        let _ = fs::create_dir_all(&fetched).and_then(|_| fs::write(&state_file, json));
    }
    println!(
        "{ready} up to date, {downloaded} fetched, {} failed",
        failed.len()
    );
    if !pins.is_empty() {
        println!("Not pinned yet; add these to the manifests:");
        for p in &pins {
            println!("  {p}");
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        for f in &failed {
            eprintln!("  {f}");
        }
        Err(format!(
            "{} files failed. If the original host is blocked here, they arrive once the Assets workflow has mirrored them.",
            failed.len()
        ))
    }
}

/// Make `target` the pinned file. Returns whether anything had to be copied.
fn place_pinned(
    e: &Entry,
    target: &Path,
    cache: &Path,
    mirror: &mut Mirror,
    placed: &mut Placed,
) -> Result<bool, String> {
    let fresh = placed
        .files
        .get(&e.path)
        .is_some_and(|p| Some(p) == PlacedFile::of(target, &e.sha256).as_ref());
    if fresh {
        return Ok(false);
    }
    if target.exists() && sha256_file(target).is_ok_and(|h| h == e.sha256) {
        if let Some(p) = PlacedFile::of(target, &e.sha256) {
            placed.files.insert(e.path.clone(), p);
        }
        return Ok(false);
    }
    let cached = cache.join(&e.sha256);
    // A damaged cache entry is downloaded again.
    if cached.exists() && !sha256_file(&cached).is_ok_and(|h| h == e.sha256) {
        let _ = fs::remove_file(&cached);
    }
    if !cached.exists() {
        let part = cache.join(format!("{}.part", e.sha256));
        let mut errors = Vec::new();
        let mut got = None;
        if let Some(url) = mirror.url_of(&e.sha256) {
            match download(&url, &part, true).and_then(|_| hash(&part)) {
                Ok(h) => got = Some(h),
                Err(why) => errors.push(format!("mirror: {why}")),
            }
        }
        if got.as_deref() != Some(e.sha256.as_str()) && e.is_made_here() {
            errors.push(if mirror.reachable() {
                "made here, and the mirror doesn't have it: whoever has the file runs `cargo run -p fetch-assets -- publish`".to_string()
            } else {
                "made here, so only the mirror has it: log in to GitHub (`gh auth login`) or set GITHUB_TOKEN".to_string()
            });
        } else if got.as_deref() != Some(e.sha256.as_str()) {
            match download(&e.url, &part, false).and_then(|_| hash(&part)) {
                Ok(h) => got = Some(h),
                Err(why) => errors.push(format!("{}: {why}", e.url)),
            }
        }
        match got {
            Some(h) if h == e.sha256 => {
                fs::rename(&part, &cached).map_err(|err| err.to_string())?;
            }
            Some(h) => {
                let _ = fs::remove_file(&part);
                return Err(format!(
                    "downloaded file has sha256 {h}, the manifest pins {}; the file changed upstream, so check it and update the pin",
                    e.sha256
                ));
            }
            None => {
                let _ = fs::remove_file(&part);
                return Err(errors.join("; "));
            }
        }
    }
    copy_into_place(&cached, target)?;
    if let Some(p) = PlacedFile::of(target, &e.sha256) {
        placed.files.insert(e.path.clone(), p);
    }
    Ok(true)
}

/// Download an unpinned file straight from its URL, place it, and return its sha256.
fn place_unpinned(e: &Entry, target: &Path, cache: &Path) -> Result<String, String> {
    let part = cache.join(format!("unpinned-{}.part", std::process::id()));
    download(&e.url, &part, false)?;
    let sha = hash(&part)?;
    let cached = cache.join(&sha);
    fs::rename(&part, &cached).map_err(|err| err.to_string())?;
    copy_into_place(&cached, target)?;
    Ok(sha)
}

fn copy_into_place(from: &Path, target: &Path) -> Result<(), String> {
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    }
    // A copy, not a link: editing the placed file must not change the cache.
    fs::copy(from, target)
        .map(|_| ())
        .map_err(|e| format!("can't write {}: {e}", target.display()))
}

fn hash(path: &Path) -> Result<String, String> {
    sha256_file(path).map_err(|e| format!("can't read {}: {e}", path.display()))
}

/// The GitHub token, if one is around (the mirror of a private repository needs it).
fn github_token() -> Option<String> {
    for key in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(t) = std::env::var(key)
            && !t.trim().is_empty()
        {
            return Some(t.trim().to_string());
        }
    }
    let out = Command::new("gh").args(["auth", "token"]).output().ok()?;
    let t = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !t.is_empty()).then_some(t)
}

/// Download with curl (ships with Windows 10+, macOS and Linux, and honors HTTPS_PROXY).
/// GitHub API requests send the token through stdin so it never shows in a process list.
fn download(url: &str, out: &Path, github_asset: bool) -> Result<(), String> {
    let mut headers = String::new();
    if github_asset {
        headers.push_str("Accept: application/octet-stream\n");
        if let Some(t) = github_token() {
            headers.push_str(&format!("Authorization: Bearer {t}\n"));
        }
    }
    curl(url, Some(out), &headers, &[]).map(|_| ())
}

fn curl(url: &str, out: Option<&Path>, headers: &str, extra: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new("curl");
    cmd.args([
        "-fsSL",
        "--retry",
        "3",
        "--retry-delay",
        "2",
        "--connect-timeout",
        "30",
    ]);
    if let Some(out) = out {
        cmd.arg("-o").arg(out);
    }
    if !headers.is_empty() {
        cmd.args(["-H", "@-"]);
    }
    cmd.args(extra)
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("can't run curl ({e}); install curl"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(headers.as_bytes());
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[derive(Deserialize)]
struct Release {
    id: u64,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    id: u64,
    name: String,
}

/// The release mirror: content-addressed copies named by sha256. Listed on first use.
struct Mirror {
    repo: String,
    assets: Option<HashMap<String, u64>>,
    reachable: bool,
}

impl Mirror {
    fn new() -> Self {
        let repo = std::env::var("ENGINE_ASSET_MIRROR")
            .ok()
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| DEFAULT_MIRROR_REPO.to_string());
        Self {
            repo,
            assets: None,
            reachable: false,
        }
    }

    fn headers(accept: &str) -> String {
        let mut headers = format!("Accept: {accept}\n");
        if let Some(t) = github_token() {
            headers.push_str(&format!("Authorization: Bearer {t}\n"));
        }
        headers
    }

    fn api(&self, path: &str) -> Result<Vec<u8>, String> {
        curl(
            &format!("https://api.github.com/repos/{}{path}", self.repo),
            None,
            &Self::headers("application/vnd.github+json"),
            &[],
        )
    }

    /// Whether the listing reached the mirror.
    fn reachable(&mut self) -> bool {
        self.listing();
        self.reachable
    }

    /// Asset name → id. Empty (with a note) when the mirror can't be reached.
    fn listing(&mut self) -> &HashMap<String, u64> {
        if self.assets.is_none() {
            let listed = match self.list() {
                Ok(listed) => {
                    self.reachable = true;
                    listed
                }
                Err(why) => {
                    eprintln!("note: release mirror unavailable ({why}); using original URLs");
                    HashMap::new()
                }
            };
            self.assets = Some(listed);
        }
        self.assets.as_ref().unwrap()
    }

    fn release_id(&self) -> Result<u64, String> {
        let release: Release =
            serde_json::from_slice(&self.api(&format!("/releases/tags/{MIRROR_TAG}"))?)
                .map_err(|e| e.to_string())?;
        Ok(release.id)
    }

    fn list(&self) -> Result<HashMap<String, u64>, String> {
        let release = self.release_id()?;
        let mut assets = HashMap::new();
        for page in 1.. {
            let batch: Vec<ReleaseAsset> = serde_json::from_slice(&self.api(&format!(
                "/releases/{release}/assets?per_page=100&page={page}"
            ))?)
            .map_err(|e| e.to_string())?;
            let n = batch.len();
            assets.extend(batch.into_iter().map(|a| (a.name, a.id)));
            if n < 100 {
                break;
            }
        }
        Ok(assets)
    }

    /// Upload `file` as the asset `name` of the mirror release.
    fn upload(&self, release: u64, file: &Path, name: &str) -> Result<(), String> {
        let mut headers = Self::headers("application/vnd.github+json");
        headers.push_str("Content-Type: application/octet-stream\n");
        let body = format!("@{}", file.display());
        curl(
            &format!(
                "https://uploads.github.com/repos/{}/releases/{release}/assets?name={name}",
                self.repo
            ),
            None,
            &headers,
            &["-X", "POST", "--data-binary", &body],
        )
        .map(|_| ())
    }

    fn url_of(&mut self, sha256: &str) -> Option<String> {
        let id = *self.listing().get(sha256)?;
        Some(format!(
            "https://api.github.com/repos/{}/releases/assets/{id}",
            self.repo
        ))
    }
}

/// CI: make sure the mirror has every listed file, and print the pins of unpinned ones.
/// Needs `gh` and a token that may write releases (the Assets workflow has both).
fn mirror(root: &Path) -> Result<(), String> {
    let manifests = load_manifests(root)?;
    let problems = validate(&manifests, false);
    if !problems.is_empty() {
        for p in &problems {
            eprintln!("  {p}");
        }
        return Err("fix the manifests first".into());
    }
    let mirror = Mirror::new();
    let repo = mirror.repo.clone();
    let gh = |args: &[&str]| -> Result<(), String> {
        let out = Command::new("gh")
            .args(args)
            .args(["-R", &repo])
            .output()
            .map_err(|e| format!("can't run gh: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    };
    if gh(&["release", "view", MIRROR_TAG]).is_err() {
        gh(&[
            "release",
            "create",
            MIRROR_TAG,
            "--prerelease",
            "--title",
            "Asset mirror",
            "--notes",
            "Copies of the third-party art listed in assets/manifests/, named by SHA-256. \
             Filled by the Assets workflow and read by `cargo run -p fetch-assets`. Don't edit by hand.",
        ])?;
    }
    let mut have: HashSet<String> = mirror.list()?.into_keys().collect();
    let work = std::env::temp_dir().join(format!("fetch-assets-mirror-{}", std::process::id()));
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let (mut uploaded, mut errors, mut pins) = (0, Vec::new(), Vec::new());
    for m in &manifests {
        for e in &m.files {
            let at = format!("{} ({})", m.name, e.path);
            if e.is_pinned() && have.contains(&e.sha256) {
                continue;
            }
            if e.is_made_here() {
                errors.push(format!(
                    "{at}: made here and not on the mirror yet; whoever has the file puts it in assets/fetched/ and runs `cargo run -p fetch-assets -- publish`"
                ));
                continue;
            }
            let part = work.join("download.part");
            let sha = match download(&e.url, &part, false).and_then(|_| hash(&part)) {
                Ok(sha) => sha,
                Err(why) => {
                    errors.push(format!("{at}: {why}"));
                    continue;
                }
            };
            if e.is_pinned() && sha != e.sha256 {
                errors.push(format!(
                    "{at}: the URL now serves sha256 {sha}, the manifest pins {}",
                    e.sha256
                ));
                continue;
            }
            if !e.is_pinned() {
                pins.push(format!("{at}: sha256 = \"{sha}\""));
            }
            if have.contains(&sha) {
                continue;
            }
            let named: PathBuf = work.join(&sha);
            fs::rename(&part, &named).map_err(|err| err.to_string())?;
            match gh(&[
                "release",
                "upload",
                MIRROR_TAG,
                &named.to_string_lossy(),
                "--clobber",
            ]) {
                Ok(()) => {
                    uploaded += 1;
                    have.insert(sha);
                }
                Err(why) => errors.push(format!("{at}: upload failed: {why}")),
            }
            let _ = fs::remove_file(&named);
        }
    }
    let _ = fs::remove_dir_all(&work);

    let mut report = format!(
        "Asset mirror: {uploaded} uploaded, {} errors\n",
        errors.len()
    );
    if !pins.is_empty() {
        report.push_str("\nNot pinned yet; add these to the manifests:\n\n");
        for p in &pins {
            report.push_str(&format!("    {p}\n"));
        }
    }
    for e in &errors {
        report.push_str(&format!("\nerror: {e}\n"));
    }
    print!("{report}");
    if let Ok(summary) = std::env::var("GITHUB_STEP_SUMMARY") {
        let _ = fs::OpenOptions::new()
            .append(true)
            .open(summary)
            .and_then(|mut f| writeln!(f, "```\n{report}```"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("{} files could not be mirrored", errors.len()))
    }
}

/// Branches that carry files made here to CI, which uploads them and deletes the branch.
const UPLOAD_BRANCH_PREFIX: &str = "asset-upload/";

/// Upload the files made here (`url = "mirror"`) that sit in `assets/fetched/` and the mirror
/// lacks, and print the pins to paste. Content-addressed, so running it again is harmless.
///
/// Where the GitHub upload endpoint is blocked (the project's cloud sessions only allow git
/// pushes), the files go on a short-lived `asset-upload/…` branch instead; the Asset upload
/// workflow copies them to the mirror and deletes the branch.
fn publish(root: &Path, only: &[String]) -> Result<(), String> {
    let manifests = selected_manifests(root, only)?;
    let fetched = fetched_dir(root);
    let mirror = Mirror::new();
    let release = mirror.release_id().map_err(|why| {
        format!(
            "can't reach the {MIRROR_TAG} release on {}: {why}",
            mirror.repo
        )
    })?;
    let mut have: HashSet<String> = mirror.list()?.into_keys().collect();
    let (mut uploaded, mut ready, mut errors, mut pins) = (0, 0, Vec::new(), Vec::new());
    // Set once a direct upload fails; the rest then go on an upload branch.
    let mut blocked: Option<String> = None;
    let mut by_branch: Vec<String> = Vec::new();
    for m in &manifests {
        for e in m.files.iter().filter(|e| e.is_made_here()) {
            let at = format!("{} ({})", m.name, e.path);
            let file = fetched.join(&e.path);
            if !file.exists() {
                if !(e.is_pinned() && have.contains(&e.sha256)) {
                    errors.push(format!("{at}: put the file at {}", file.display()));
                }
                continue;
            }
            let sha = hash(&file)?;
            if !e.is_pinned() {
                pins.push(format!("{at}: sha256 = \"{sha}\""));
            } else if sha != e.sha256 {
                pins.push(format!(
                    "{at}: the file changed; new sha256 = \"{sha}\" (pinned {})",
                    e.sha256
                ));
            }
            if have.contains(&sha) {
                ready += 1;
                continue;
            }
            if blocked.is_none() {
                match mirror.upload(release, &file, &sha) {
                    Ok(()) => {
                        uploaded += 1;
                        have.insert(sha);
                        continue;
                    }
                    Err(why) => blocked = Some(why),
                }
            }
            by_branch.push(format!("assets/fetched/{}", e.path));
            have.insert(sha);
        }
    }
    let mut branch = None;
    if !by_branch.is_empty() {
        let why = blocked.unwrap_or_default();
        // CI is where upload branches end up; pushing another from there would loop.
        let pushed = if std::env::var_os("GITHUB_ACTIONS").is_some() {
            Err("not from CI".to_string())
        } else {
            push_upload_branch(root, &by_branch)
        };
        match pushed {
            Ok(name) => branch = Some(name),
            Err(push) => errors.push(format!(
                "{} files: direct upload failed ({why}) and so did pushing an upload branch ({push})",
                by_branch.len()
            )),
        }
    }
    println!(
        "{uploaded} uploaded, {} sent on an upload branch, {ready} already on the mirror, {} failed",
        if branch.is_some() { by_branch.len() } else { 0 },
        errors.len()
    );
    if let Some(branch) = &branch {
        println!(
            "Direct uploads are blocked here, so the files went to branch {branch}. The \"Asset upload\" \
             workflow copies them to the mirror and deletes the branch; push the pins after it has run."
        );
    }
    if !pins.is_empty() {
        println!("Paste these pins into the manifests:");
        for p in &pins {
            println!("  {p}");
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        for e in &errors {
            eprintln!("  {e}");
        }
        Err(format!("{} files not published", errors.len()))
    }
}

/// Push a commit with `files` (force-added, they are gitignored) and the working tree's
/// manifests on top of HEAD to a new `asset-upload/…` branch. Uses a scratch index, so the
/// checkout, its index and its branch are left alone. Returns the branch name.
fn push_upload_branch(root: &Path, files: &[String]) -> Result<String, String> {
    let index = std::env::temp_dir().join(format!("fetch-assets-index-{}", std::process::id()));
    let git = |args: &[&str]| -> Result<String, String> {
        let out = Command::new("git")
            .current_dir(root)
            .env("GIT_INDEX_FILE", &index)
            .args(args)
            .output()
            .map_err(|e| format!("can't run git: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            Err(format!(
                "git {}: {}",
                args.first().unwrap_or(&""),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    };
    let result = (|| {
        git(&["read-tree", "HEAD"])?;
        let mut add = vec!["add", "-f", "--", "assets/manifests"];
        add.extend(files.iter().map(String::as_str));
        git(&add)?;
        let tree = git(&["write-tree"])?;
        let message = format!(
            "Asset upload: {} files made here\n\nFor the mirror only; the Asset upload workflow deletes this branch.",
            files.len()
        );
        let commit = git(&["commit-tree", &tree, "-p", "HEAD", "-m", &message])?;
        let branch = format!("{UPLOAD_BRANCH_PREFIX}{}", &commit[..12.min(commit.len())]);
        git(&[
            "push",
            "-q",
            "origin",
            &format!("{commit}:refs/heads/{branch}"),
        ])?;
        Ok(branch)
    })();
    let _ = fs::remove_file(&index);
    result
}
