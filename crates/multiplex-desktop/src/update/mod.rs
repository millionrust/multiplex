//! Updating the desktop app from GitHub Releases.
//!
//! The app asks the releases API for anything newer than itself, downloads the package for this
//! platform in the background, checks it against the SHA-256 file published beside it, and keeps
//! it staged until the person chooses "Restart to Update". Then [`apply`] runs as a separate
//! process from the old copy: it waits for the app to exit, puts the new version in place, and
//! opens it.
//!
//! What updates itself: the macOS app (the universal zip) and a Windows copy installed from the
//! per-user MSI. A portable Windows zip and Linux are told about the update and pointed at the
//! release page, since replacing them needs the person (a `.deb` needs `sudo`). Phones update
//! through their stores and are not handled here. Development builds never update themselves.
//!
//! The checksum proves the download arrived intact, not who built it: it comes from the same
//! release. `docs/decisions/update-trust.md` is the route to signed update metadata.

pub mod apply;

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub const RELEASES_PAGE: &str = "https://github.com/millionrust/multiplex/releases";
const RELEASES_API: &str =
    "https://api.github.com/repos/millionrust/multiplex/releases?per_page=20";
/// Every download must come from this repository's release assets.
const DOWNLOAD_PREFIX: &str = "https://github.com/millionrust/multiplex/releases/download/";
/// The command line that hands a staged update to [`apply::run`].
pub const APPLY_UPDATE_COMMAND: &str = "--apply-update";
const STAGED_RECORD: &str = "staged.json";
const MAX_RELEASES_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CHECKSUM_BYTES: u64 = 16 * 1024;
const MAX_PACKAGE_BYTES: u64 = 600 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// `0.0.4` or `v0.0.4`. Anything with a suffix such as `-rc1` is not offered.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.strip_prefix('v').unwrap_or(text).split('.');
        let version = Self(
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        );
        parts.next().is_none().then_some(version)
    }

    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Self(0, 0, 0))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// How a downloaded package is put in place.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Installer {
    /// The universal zip, unpacked over the running `Multiplex.app`.
    MacAppZip,
    /// The per-user MSI, run with `msiexec`.
    WindowsMsi,
}

/// The release file this copy of the app updates from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Package {
    pub asset: &'static str,
    pub checksum: &'static str,
    pub installer: Installer,
}

/// What this copy can update itself from, or `None` when the person has to do it: Linux, a
/// portable Windows copy, or an app that is not inside an installed bundle.
pub fn package_for_this_copy() -> Option<Package> {
    let executable = std::env::current_exe().ok()?;
    package_for(
        std::env::consts::OS,
        std::env::consts::ARCH,
        &executable,
        dirs::data_local_dir().as_deref(),
    )
}

fn package_for(
    os: &str,
    arch: &str,
    executable: &Path,
    local_data: Option<&Path>,
) -> Option<Package> {
    match (os, arch) {
        ("macos", _) => mac_bundle(executable).map(|_| Package {
            asset: "Multiplex-macos-universal.zip",
            checksum: "Multiplex-macos-universal.zip.sha256",
            installer: Installer::MacAppZip,
        }),
        ("windows", "x86_64" | "aarch64") => {
            // Only the MSI's own folder is replaced by the MSI; a portable copy is left alone.
            let installed = local_data?.join("Programs").join("Multiplex");
            if !executable.starts_with(&installed) {
                return None;
            }
            Some(if arch == "x86_64" {
                Package {
                    asset: "Multiplex-windows-x86_64.msi",
                    checksum: "Multiplex-windows-x86_64.msi.sha256",
                    installer: Installer::WindowsMsi,
                }
            } else {
                Package {
                    asset: "Multiplex-windows-aarch64.msi",
                    checksum: "Multiplex-windows-aarch64.msi.sha256",
                    installer: Installer::WindowsMsi,
                }
            })
        }
        _ => None,
    }
}

/// The `.app` bundle an executable runs from, when it runs from one.
pub fn mac_bundle(executable: &Path) -> Option<PathBuf> {
    executable
        .ancestors()
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
        .map(Path::to_path_buf)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Download {
    pub url: String,
    pub size: u64,
}

/// A newer release, and what to download for this copy when it can update itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Offer {
    pub version: Version,
    pub page: String,
    pub package: Option<(Package, Download, Download)>,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    draft: bool,
    html_url: String,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Debug)]
pub enum UpdateError {
    Network,
    UnexpectedResponse,
    TooLarge,
    ChecksumMismatch,
    Storage,
}

/// The newest published release above `current`, from the releases API's answer. Prereleases
/// count: every release so far is one.
pub fn choose(
    releases: &[u8],
    current: Version,
    package: Option<Package>,
) -> Result<Option<Offer>, UpdateError> {
    let releases: Vec<ApiRelease> =
        serde_json::from_slice(releases).map_err(|_| UpdateError::UnexpectedResponse)?;
    let newest = releases
        .into_iter()
        .filter(|release| !release.draft)
        .filter_map(|release| Some((Version::parse(&release.tag_name)?, release)))
        .filter(|(version, _)| *version > current)
        .max_by_key(|(version, _)| *version);
    let Some((version, release)) = newest else {
        return Ok(None);
    };
    let find = |name: &str| {
        release
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .filter(|asset| asset.browser_download_url.starts_with(DOWNLOAD_PREFIX))
            .map(|asset| Download {
                url: asset.browser_download_url.clone(),
                size: asset.size,
            })
    };
    let package =
        package.and_then(|package| Some((package, find(package.asset)?, find(package.checksum)?)));
    let page = if release.html_url.starts_with(RELEASES_PAGE) {
        release.html_url
    } else {
        RELEASES_PAGE.to_owned()
    };
    Ok(Some(Offer {
        version,
        page,
        package,
    }))
}

/// The digest a `.sha256` file gives for `asset`: `<hex>  <name>` lines, as `shasum` writes.
pub fn expected_digest(checksums: &str, asset: &str) -> Option<[u8; 32]> {
    checksums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hex = fields.next()?;
        let name = fields.next()?.trim_start_matches('*');
        if name != asset || hex.len() != 64 {
            return None;
        }
        let mut digest = [0_u8; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(hex.get(index * 2..index * 2 + 2)?, 16).ok()?;
        }
        Some(digest)
    })
}

fn client(timeout: Duration) -> Result<reqwest::blocking::Client, UpdateError> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("Multiplex/", env!("CARGO_PKG_VERSION")))
        .timeout(timeout)
        .build()
        .map_err(|_| UpdateError::Network)
}

fn read_bounded(response: reqwest::blocking::Response, limit: u64) -> Result<Vec<u8>, UpdateError> {
    let mut body = Vec::new();
    response
        .take(limit + 1)
        .read_to_end(&mut body)
        .map_err(|_| UpdateError::Network)?;
    if body.len() as u64 > limit {
        return Err(UpdateError::TooLarge);
    }
    Ok(body)
}

fn get(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<reqwest::blocking::Response, UpdateError> {
    let response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|_| UpdateError::Network)?;
    if !response.status().is_success() {
        return Err(UpdateError::UnexpectedResponse);
    }
    Ok(response)
}

/// Asks GitHub whether there is anything newer than this copy.
pub fn check() -> Result<Option<Offer>, UpdateError> {
    let client = client(REQUEST_TIMEOUT)?;
    let body = read_bounded(get(&client, RELEASES_API)?, MAX_RELEASES_BYTES)?;
    let mut offer = choose(&body, Version::current(), package_for_this_copy())?;
    // A version whose install failed is offered as its release page, not downloaded again.
    if let Some(offer) = offer.as_mut()
        && staging_dir().and_then(|dir| apply::failed_version(&dir)) == Some(offer.version)
    {
        offer.package = None;
    }
    Ok(offer)
}

/// A downloaded, checked package waiting for a restart.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Staged {
    pub version: Version,
    pub installer: Installer,
    pub path: PathBuf,
}

/// Where packages wait: `<app dir>/updates`.
pub fn staging_dir() -> Option<PathBuf> {
    crate::storage::app_dir()
        .ok()
        .map(|dir| dir.join("updates"))
}

/// Downloads the offer's package into `dir`, checks its SHA-256, and records it as staged.
pub fn download(offer: &Offer, dir: &Path) -> Result<Staged, UpdateError> {
    let Some((package, file, checksum)) = &offer.package else {
        return Err(UpdateError::UnexpectedResponse);
    };
    if file.size == 0 || file.size > MAX_PACKAGE_BYTES || checksum.size > MAX_CHECKSUM_BYTES {
        return Err(UpdateError::TooLarge);
    }
    let client = client(DOWNLOAD_TIMEOUT)?;
    let checksums = read_bounded(get(&client, &checksum.url)?, MAX_CHECKSUM_BYTES)?;
    let expected = expected_digest(&String::from_utf8_lossy(&checksums), package.asset)
        .ok_or(UpdateError::UnexpectedResponse)?;

    // Anything staged before is replaced: only the newest update is kept.
    let _ = fs::remove_dir_all(dir);
    let target_dir = dir.join(offer.version.to_string());
    fs::create_dir_all(&target_dir).map_err(|_| UpdateError::Storage)?;
    let partial = target_dir.join(format!("{}.part", package.asset));
    let path = target_dir.join(package.asset);
    let mut output = fs::File::create(&partial).map_err(|_| UpdateError::Storage)?;
    let mut response = get(&client, &file.url)?.take(file.size + 1);
    let mut hash = Sha256::new();
    let mut written = 0_u64;
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|_| UpdateError::Network)?;
        if read == 0 {
            break;
        }
        written += read as u64;
        if written > file.size {
            return Err(UpdateError::TooLarge);
        }
        hash.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .map_err(|_| UpdateError::Storage)?;
    }
    output.sync_all().map_err(|_| UpdateError::Storage)?;
    drop(output);
    if written != file.size || hash.finalize().as_slice() != expected {
        let _ = fs::remove_file(&partial);
        return Err(UpdateError::ChecksumMismatch);
    }
    fs::rename(&partial, &path).map_err(|_| UpdateError::Storage)?;
    let staged = Staged {
        version: offer.version,
        installer: package.installer,
        path,
    };
    let record = serde_json::to_vec_pretty(&staged).map_err(|_| UpdateError::Storage)?;
    fs::write(dir.join(STAGED_RECORD), record).map_err(|_| UpdateError::Storage)?;
    Ok(staged)
}

/// The update staged in `dir`, when it is still newer than this copy and its file is there.
pub fn staged(dir: &Path) -> Option<Staged> {
    let record = fs::read(dir.join(STAGED_RECORD)).ok()?;
    let staged: Staged = serde_json::from_slice(&record).ok()?;
    (staged.version > Version::current() && staged.path.starts_with(dir) && staged.path.is_file())
        .then_some(staged)
}

/// Removes whatever is staged, such as after the update was installed.
pub fn clear_staged(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// Starts the updater from this copy of the app and returns; the caller then quits, and the
/// updater waits for that before touching anything.
pub fn launch_apply(staged: &Staged) -> std::io::Result<()> {
    let executable = std::env::current_exe()?;
    let mut command = std::process::Command::new(executable);
    command
        .arg(APPLY_UPDATE_COMMAND)
        .arg(&staged.path)
        .arg(staged.version.to_string())
        .arg(std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command.spawn().map(drop)
}

/// Whether this build looks for updates at all: never in development or test builds, and never
/// when `MULTIPLEX_DISABLE_UPDATES` is set.
pub fn updates_enabled() -> bool {
    !cfg!(debug_assertions)
        && !cfg!(test)
        && multiplex_env::var_os("MULTIPLEX_DISABLE_UPDATES").is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: Package = Package {
        asset: "Multiplex-macos-universal.zip",
        checksum: "Multiplex-macos-universal.zip.sha256",
        installer: Installer::MacAppZip,
    };

    fn release(tag: &str, draft: bool, assets: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag,
            "draft": draft,
            "prerelease": true,
            "html_url": format!("{RELEASES_PAGE}/tag/{tag}"),
            "assets": assets.iter().map(|name| serde_json::json!({
                "name": name,
                "browser_download_url": format!("{DOWNLOAD_PREFIX}{tag}/{name}"),
                "size": 10,
            })).collect::<Vec<_>>(),
        })
    }

    fn body(releases: Vec<serde_json::Value>) -> Vec<u8> {
        serde_json::to_vec(&releases).unwrap()
    }

    #[test]
    fn versions_parse_with_or_without_a_v_and_order_numerically() {
        assert_eq!(Version::parse("v0.0.10"), Some(Version(0, 0, 10)));
        assert_eq!(Version::parse("1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("0.1"), None);
        assert_eq!(Version::parse("0.1.0-rc1"), None);
        assert_eq!(Version::parse("0.1.0.0"), None);
        assert!(Version(0, 0, 10) > Version(0, 0, 9));
    }

    #[test]
    fn the_newest_published_release_above_this_one_is_offered() {
        let offer = choose(
            &body(vec![
                release("v0.0.3", false, &[MAC.asset, MAC.checksum]),
                release("v0.0.5", true, &[MAC.asset, MAC.checksum]),
                release("v0.0.4", false, &[MAC.asset, MAC.checksum]),
                release("nightly", false, &[]),
            ]),
            Version(0, 0, 3),
            Some(MAC),
        )
        .unwrap()
        .unwrap();
        assert_eq!(offer.version, Version(0, 0, 4));
        assert_eq!(offer.page, format!("{RELEASES_PAGE}/tag/v0.0.4"));
        let (_, file, checksum) = offer.package.unwrap();
        assert!(file.url.ends_with("/v0.0.4/Multiplex-macos-universal.zip"));
        assert!(checksum.url.ends_with(".zip.sha256"));
    }

    #[test]
    fn nothing_is_offered_when_this_copy_is_current() {
        let offered = choose(
            &body(vec![release("v0.0.3", false, &[MAC.asset, MAC.checksum])]),
            Version(0, 0, 3),
            Some(MAC),
        )
        .unwrap();
        assert_eq!(offered, None);
    }

    #[test]
    fn a_release_missing_this_platforms_files_is_offered_as_a_page_only() {
        let offer = choose(
            &body(vec![release("v0.0.4", false, &[MAC.asset])]),
            Version(0, 0, 3),
            Some(MAC),
        )
        .unwrap()
        .unwrap();
        assert_eq!(offer.package, None);
    }

    #[test]
    fn a_download_from_anywhere_but_this_repository_is_refused() {
        let mut hostile = release("v0.0.4", false, &[MAC.asset, MAC.checksum]);
        hostile["assets"][0]["browser_download_url"] =
            serde_json::json!("https://example.com/Multiplex-macos-universal.zip");
        let offer = choose(&body(vec![hostile]), Version(0, 0, 3), Some(MAC))
            .unwrap()
            .unwrap();
        assert_eq!(offer.package, None);
    }

    #[test]
    fn the_checksum_file_is_read_as_shasum_writes_it() {
        let hex = "ab".repeat(32);
        let text = format!(
            "{}  Multiplex-linux-x86_64.tar.gz\n{hex}  Multiplex-macos-universal.zip\n",
            "cd".repeat(32)
        );
        assert_eq!(
            expected_digest(&text, "Multiplex-macos-universal.zip"),
            Some([0xab; 32])
        );
        assert_eq!(expected_digest(&text, "missing.zip"), None);
        assert_eq!(expected_digest("zz  a.zip", "a.zip"), None);
    }

    #[test]
    fn only_installed_copies_update_themselves() {
        let local = Path::new("C:/Users/me/AppData/Local");
        assert_eq!(
            package_for(
                "windows",
                "x86_64",
                &local.join("Programs/Multiplex/multiplex.exe"),
                Some(local)
            )
            .map(|package| package.asset),
            Some("Multiplex-windows-x86_64.msi")
        );
        assert_eq!(
            package_for(
                "windows",
                "aarch64",
                &local.join("Programs/Multiplex/multiplex.exe"),
                Some(local)
            )
            .map(|package| package.asset),
            Some("Multiplex-windows-aarch64.msi")
        );
        assert_eq!(
            package_for(
                "windows",
                "x86_64",
                Path::new("D:/tools/Multiplex/multiplex.exe"),
                Some(local)
            ),
            None
        );
        assert_eq!(
            package_for(
                "macos",
                "aarch64",
                Path::new("/Applications/Multiplex.app/Contents/MacOS/multiplex"),
                None
            ),
            Some(MAC)
        );
        assert_eq!(
            package_for(
                "macos",
                "aarch64",
                Path::new("/Users/me/multiplex/target/release/multiplex"),
                None
            ),
            None
        );
        assert_eq!(
            package_for("linux", "x86_64", Path::new("/usr/bin/multiplex"), None),
            None
        );
    }

    #[test]
    fn a_staged_update_is_kept_only_while_it_is_newer_and_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("0.0.99/Multiplex-macos-universal.zip");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"zip").unwrap();
        let record = Staged {
            version: Version(0, 0, 99),
            installer: Installer::MacAppZip,
            path: path.clone(),
        };
        fs::write(
            dir.path().join(STAGED_RECORD),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        assert_eq!(staged(dir.path()), Some(record.clone()));

        let old = Staged {
            version: Version(0, 0, 0),
            ..record.clone()
        };
        fs::write(
            dir.path().join(STAGED_RECORD),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        assert_eq!(staged(dir.path()), None);

        let elsewhere = Staged {
            path: PathBuf::from("/tmp/elsewhere.zip"),
            ..record
        };
        fs::write(
            dir.path().join(STAGED_RECORD),
            serde_json::to_vec(&elsewhere).unwrap(),
        )
        .unwrap();
        assert_eq!(staged(dir.path()), None);
    }
}
