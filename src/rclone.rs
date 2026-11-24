//! Storage read and written through rclone (`type = "rclone"`): packs on a Google Drive
//! shared drive, fetched with each user's own Google account. Who may fetch them is the
//! drive's sharing; the SHA-256 in the manifest is what makes the bytes trustworthy.
//!
//! rclone runs as a child process. Each user has one remote that holds their Google
//! credentials (`carla-pack drive login` makes it, `carla-pack-gdrive` by default); the
//! drive and folder come from the manifest and are set on every call with rclone's
//! connection-string options (`remote,team_drive='…',root_folder_id='…':path`), so the
//! same files are read whatever that remote's own configuration says. Unattended runs
//! can use a Google service account the drive is shared with instead of a remote.
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;

use crate::error::{Error, Result};
use crate::manifest::RcloneStorage;

/// The remote `carla-pack drive login` creates.
pub const DEFAULT_REMOTE: &str = "carla-pack-gdrive";

/// How much of rclone's error output goes into a message.
const STDERR_LINES: usize = 4;
/// How many of its last lines are kept while a download runs (its retries log too).
const STDERR_KEPT: usize = 16;

/// How carla-pack runs rclone.
#[derive(Debug, Clone)]
pub struct Rclone {
    /// The rclone executable: `CARLA_PACK_RCLONE`, else `rclone` on PATH.
    pub binary: PathBuf,
    /// The remote holding the user's credentials: `CARLA_PACK_RCLONE_REMOTE`, else
    /// [`DEFAULT_REMOTE`].
    pub remote: String,
    /// A Google service account key used instead of the remote (CI):
    /// `CARLA_PACK_RCLONE_SERVICE_ACCOUNT_FILE`.
    pub service_account_file: Option<PathBuf>,
    /// Extra environment for rclone (tests point `RCLONE_CONFIG*` at their own remotes).
    pub env: Vec<(OsString, OsString)>,
}

impl Default for Rclone {
    fn default() -> Self {
        Self { binary: "rclone".into(), remote: DEFAULT_REMOTE.into(), service_account_file: None, env: Vec::new() }
    }
}

/// Whether a call reads or writes (a service account gets the matching OAuth scope).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    Write,
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// rclone remote names: letters, digits and `_ - . + @` (no spaces, no leading `-`).
fn is_remote_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'+' | b'@'))
}

/// A connection-string value, single-quoted (a quote inside is doubled).
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

impl Rclone {
    /// Settings from the environment (`CARLA_PACK_RCLONE`, `CARLA_PACK_RCLONE_REMOTE`,
    /// `CARLA_PACK_RCLONE_SERVICE_ACCOUNT_FILE`).
    pub fn from_env() -> Self {
        Self {
            binary: env_var("CARLA_PACK_RCLONE").map(PathBuf::from).unwrap_or_else(|| "rclone".into()),
            remote: env_var("CARLA_PACK_RCLONE_REMOTE")
                .map(|r| r.trim_end_matches(':').to_string())
                .unwrap_or_else(|| DEFAULT_REMOTE.into()),
            service_account_file: env_var("CARLA_PACK_RCLONE_SERVICE_ACCOUNT_FILE").map(PathBuf::from),
            env: Vec::new(),
        }
    }

    /// "rclone remote carla-pack-gdrive" / "the service account in sa.json", for messages.
    pub fn identity(&self) -> String {
        match &self.service_account_file {
            Some(f) => format!("the Google service account in {}", f.display()),
            None => format!("rclone remote {}", self.remote),
        }
    }

    /// `remote,team_drive='…',root_folder_id='…':path` (or an on-the-fly `:drive,…:` remote
    /// with a service account). Both IDs are always set, empty when the manifest has none,
    /// so a remote configured for another drive or folder cannot redirect the read.
    fn spec(&self, storage: &RcloneStorage, path: &str, access: Access) -> Result<String> {
        let mut options = vec![
            format!("team_drive={}", quote(storage.team_drive.as_deref().unwrap_or(""))),
            format!("root_folder_id={}", quote(storage.root_folder_id.as_deref().unwrap_or(""))),
            "shared_with_me='false'".to_string(),
        ];
        let remote = match &self.service_account_file {
            Some(file) => {
                let file = file.to_str().ok_or_else(|| {
                    Error::Usage(format!("the service account file path {} is not UTF-8", file.display()))
                })?;
                let scope = match access {
                    Access::Read => "drive.readonly",
                    Access::Write => "drive",
                };
                options.insert(0, format!("service_account_file={}", quote(file)));
                options.insert(1, format!("scope={}", quote(scope)));
                format!(":{}", storage.backend.as_str())
            }
            None => {
                if !is_remote_name(&self.remote) {
                    return Err(Error::Usage(format!(
                        "CARLA_PACK_RCLONE_REMOTE {:?} is not an rclone remote name",
                        self.remote
                    )));
                }
                self.remote.clone()
            }
        };
        Ok(format!("{remote},{}:{path}", options.join(",")))
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.binary);
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)));
        // rclone's own retries (low-level ones included) stay on; carla-pack resumes a
        // broken download itself, from the byte it stopped at.
        cmd.stdin(Stdio::null());
        cmd
    }

    fn spawn_error(&self, e: io::Error) -> Error {
        if e.kind() == io::ErrorKind::NotFound {
            Error::RcloneUnavailable(format!(
                "this pack repository stores its packs on Google Drive, which carla-pack reads through rclone, \
                 and rclone was not found ({}); install it (https://rclone.org/install/) or set CARLA_PACK_RCLONE \
                 to its path, then run `carla-pack drive login`",
                self.binary.display()
            ))
        } else {
            Error::RcloneUnavailable(format!("cannot run rclone ({}): {e}", self.binary.display()))
        }
    }

    /// Runs rclone to completion, returning its standard output.
    fn run(&self, args: &[&str]) -> Result<std::result::Result<Vec<u8>, Failure>> {
        let out = self
            .command()
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| self.spawn_error(e))?;
        if out.status.success() {
            return Ok(Ok(out.stdout));
        }
        Ok(Err(Failure { status: out.status, stderr: String::from_utf8_lossy(&out.stderr).into_owned() }))
    }

    /// Whether there is a file at `path` (relative to the drive or folder). On Google
    /// Drive a drive that is not shared with the account is an error (404), and a missing
    /// file in a readable drive is `false`.
    fn exists(&self, storage: &RcloneStorage, path: &str, access: Access) -> Result<bool> {
        let spec = self.spec(storage, path, access)?;
        match self.run(&["lsjson", "--stat", "--", &spec])? {
            Ok(_) => Ok(true),
            Err(f) if f.is_not_found() && !f.stderr.contains("Shared drive not found") => Ok(false),
            Err(f) => Err(self.classify(&f, path, storage)),
        }
    }

    /// Checks that the drive (and folder, and prefix) can be read: what `carla-pack
    /// credentials` reports.
    pub fn check_access(&self, storage: &RcloneStorage) -> Result<()> {
        let prefix = storage.prefix.trim_end_matches('/');
        let spec = self.spec(storage, prefix, Access::Read)?;
        match self.run(&["lsjson", "--stat", "--", &spec])? {
            Ok(_) => Ok(()),
            Err(f) => Err(self.classify(&f, if prefix.is_empty() { "the top folder" } else { prefix }, storage)),
        }
    }

    /// Streams the file at `path` from byte `offset` on (`rclone cat --offset`). A file
    /// that cannot be read fails on the first read, with the classified error.
    pub fn open(&self, storage: &RcloneStorage, path: &str, offset: u64) -> Result<RcloneReader> {
        let spec = self.spec(storage, path, Access::Read)?;
        let mut cmd = self.command();
        cmd.arg("cat");
        if offset > 0 {
            cmd.arg("--offset").arg(offset.to_string());
        }
        let mut child = cmd
            .arg("--")
            .arg(&spec)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| self.spawn_error(e))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = BufReader::new(child.stderr.take().expect("piped stderr"));
        let stderr = std::thread::spawn(move || {
            let mut kept = VecDeque::with_capacity(STDERR_KEPT);
            for line in stderr.lines().map_while(std::result::Result::ok) {
                if kept.len() == STDERR_KEPT {
                    kept.pop_front();
                }
                kept.push_back(line);
            }
            Vec::from(kept).join("\n")
        });
        Ok(RcloneReader {
            child: Some(child),
            stdout,
            stderr: Some(stderr),
            rclone: self.clone(),
            storage: storage.clone(),
            path: path.to_string(),
        })
    }

    /// Uploads `local` to `path` (`rclone copyto`, which checks the uploaded file's hash
    /// against the local one itself).
    pub fn upload(&self, storage: &RcloneStorage, local: &Path, path: &str, overwrite: bool) -> Result<()> {
        if !overwrite && self.exists(storage, path, Access::Write)? {
            return Err(Error::Usage(format!(
                "{path} already exists in {}; pass overwrite to replace it",
                storage.describe()
            )));
        }
        let local_text =
            local.to_str().ok_or_else(|| Error::Usage(format!("the tarball path {} is not UTF-8", local.display())))?;
        let spec = self.spec(storage, path, Access::Write)?;
        match self.run(&["copyto", "--", local_text, &spec])? {
            Ok(_) => Ok(()),
            Err(f) => Err(self.classify(&f, path, storage)),
        }
    }

    /// A typed error for a failed rclone call about `path`.
    fn classify(&self, f: &Failure, path: &str, storage: &RcloneStorage) -> Error {
        let service_account = self.service_account_file.is_some();
        let (storage, identity, remote) = (storage.describe(), self.identity(), &self.remote);
        let lines = f.lines();
        let err = lines.join("\n");
        let has = |needles: &[&str]| needles.iter().any(|n| err.contains(n));
        let detail = Failure::summary_of(&lines, f.status);
        if !service_account && has(&["didn't find section in config file", "not found in config file"]) {
            return Error::StorageUnauthenticated(format!(
                "rclone has no remote {remote:?} for your Google account; run `carla-pack drive login` \
                 (or set CARLA_PACK_RCLONE_REMOTE to a Google Drive remote you have) [{detail}]"
            ));
        }
        if has(&[
            "oauth2: ",
            "invalid_grant",
            "unauthorized_client",
            "invalid_client",
            "Error 401",
            "empty token",
            "token expired",
        ]) {
            let fix = if service_account {
                "check the service account key file"
            } else {
                "run `carla-pack drive login` again"
            };
            return Error::StorageUnauthenticated(format!(
                "Google rejected the credentials of {identity}; {fix} [{detail}]"
            ));
        }
        if has(&["downloadQuotaExceeded", "rateLimitExceeded", "userRateLimitExceeded", "quotaExceeded", "Error 429"]) {
            return Error::Storage {
                status: 429,
                message: format!(
                    "Google Drive limited downloads of {path} in {storage} (quota or rate limit); retry later [{detail}]"
                ),
            };
        }
        if f.is_not_found()
            || has(&["notFound", "not found", "Error 404", "insufficientFilePermissions", "Error 403", "forbidden"])
        {
            return Error::StorageNoAccess(format!(
                "{path} is not readable in {storage} as {identity}: the file is missing, or the drive or folder is not \
                 shared with that Google account (Google Drive answers both the same way); ask the drive's owner to \
                 share it with you, or check `carla-pack drive login` used the right account [{detail}]"
            ));
        }
        Error::Storage { status: 0, message: format!("rclone failed on {path} in {storage}: {detail}") }
    }
}

/// A failed rclone run.
#[derive(Debug)]
struct Failure {
    status: ExitStatus,
    stderr: String,
}

impl Failure {
    /// rclone's exit codes 3 (directory not found) and 4 (file not found).
    fn is_not_found(&self) -> bool {
        matches!(self.status.code(), Some(3 | 4))
    }

    /// rclone's error output without its "config file not found - using defaults" notice
    /// (a service account, or a remote set in the environment, needs no config file).
    fn lines(&self) -> Vec<&str> {
        self.stderr
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !(l.contains("Config file") && l.contains("using defaults")))
            .collect()
    }

    /// The last lines of rclone's error output (`lines`, from [`Failure::lines`]).
    fn summary_of(lines: &[&str], status: ExitStatus) -> String {
        let tail = &lines[lines.len().saturating_sub(STDERR_LINES)..];
        if tail.is_empty() { format!("rclone exited with {status}") } else { tail.join(" / ") }
    }
}

/// `rclone cat`'s output. At the end the exit status is checked, never a silent EOF: a
/// cut-off stream is an I/O error (which the download resumes from); a refusal (access,
/// credentials, quota) carries the typed [`Error`], which ends the download.
pub struct RcloneReader {
    child: Option<Child>,
    stdout: ChildStdout,
    stderr: Option<JoinHandle<String>>,
    rclone: Rclone,
    storage: RcloneStorage,
    path: String,
}

impl Read for RcloneReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.stdout.read(buf)?;
        if n > 0 || buf.is_empty() {
            return Ok(n);
        }
        let Some(mut child) = self.child.take() else { return Ok(0) };
        let status = child.wait()?;
        let stderr = self.stderr.take().and_then(|h| h.join().ok()).unwrap_or_default();
        if status.success() {
            return Ok(0);
        }
        match self.rclone.classify(&Failure { status, stderr }, &self.path, &self.storage) {
            Error::Storage { status: 0, message } => Err(io::Error::other(message)),
            refused => Err(io::Error::other(refused)),
        }
    }
}

impl Drop for RcloneReader {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::RcloneBackend;

    fn storage(team_drive: Option<&str>, folder: Option<&str>) -> RcloneStorage {
        RcloneStorage {
            backend: RcloneBackend::Drive,
            team_drive: team_drive.map(Into::into),
            root_folder_id: folder.map(Into::into),
            prefix: "packs/".into(),
        }
    }

    #[test]
    fn pins_the_drive_and_folder_on_every_call() {
        let r = Rclone::default();
        assert_eq!(
            r.spec(&storage(Some("0ABcdEfGhIjKlUk9PVA"), None), "packs/A.tar.gz", Access::Read).unwrap(),
            "carla-pack-gdrive,team_drive='0ABcdEfGhIjKlUk9PVA',root_folder_id='',shared_with_me='false':packs/A.tar.gz"
        );
        let sa = Rclone { service_account_file: Some("/keys/it's.json".into()), ..Rclone::default() };
        assert_eq!(
            sa.spec(&storage(None, Some("1a2b3c4d5e6f")), "x", Access::Write).unwrap(),
            ":drive,service_account_file='/keys/it''s.json',scope='drive',team_drive='',root_folder_id='1a2b3c4d5e6f',\
             shared_with_me='false':x"
        );
        let bad = Rclone { remote: "evil,team_drive=x".into(), ..Rclone::default() };
        assert!(bad.spec(&storage(Some("0ABcdEfGhIjKlUk9PVA"), None), "x", Access::Read).is_err());
    }

    #[cfg(unix)]
    fn failure(code: i32, stderr: &str) -> Failure {
        use std::os::unix::process::ExitStatusExt;
        Failure { status: ExitStatus::from_raw(code << 8), stderr: stderr.into() }
    }

    #[cfg(unix)]
    #[test]
    fn maps_rclone_errors_to_codes() {
        let drive = storage(Some("0ABcdEfGhIjKlUk9PVA"), None);
        let classify = |status: i32, stderr: &str| Rclone::default().classify(&failure(status, stderr), "p", &drive);
        let code = |status: i32, stderr: &str| classify(status, stderr).code();
        assert_eq!(
            code(1, "CRITICAL: Failed to create file system: didn't find section in config file (\"r\")"),
            "storage_unauthenticated"
        );
        assert_eq!(
            code(1, "oauth2: cannot fetch token: 400 Bad Request {\"error\": \"invalid_grant\"}"),
            "storage_unauthenticated"
        );
        assert_eq!(
            code(1, "googleapi: Error 403: The download quota for this file has been exceeded., downloadQuotaExceeded"),
            "storage_error"
        );
        assert_eq!(code(1, "googleapi: Error 404: Shared drive not found: 0AB, notFound"), "storage_no_access");
        assert_eq!(code(3, "Failed to lsjson: directory not found"), "storage_no_access");
        assert_eq!(code(1, "something else"), "storage_error");
        let msg = classify(1, "NOTICE: Config file \"x\" not found - using defaults\nERROR : boom");
        assert!(msg.to_string().ends_with("shared drive 0ABcdEfGhIjKlUk9PVA: ERROR : boom"), "{msg}");
    }
}
