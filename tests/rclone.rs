//! Packs on rclone storage (`type = "rclone"`, Google Drive), against the real rclone
//! binary: an `alias` remote over a local folder plays the user's Google Drive remote
//! (rclone accepts the drive options carla-pack pins and the alias ignores them). The
//! GitHub side is mocked as in flow.rs. Skipped when rclone is not installed
//! (`CARLA_PACK_TEST_RCLONE`, else `rclone` on PATH); CI installs it.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use carla_pack_registry::client::ClientOptions;
use carla_pack_registry::manifest::Filter;
use carla_pack_registry::pack::InstallOptions;
use carla_pack_registry::rclone::Rclone;
use carla_pack_registry::store::Token;
use carla_pack_registry::{Client, Error, PublishOptions, publish};
use common::*;
use httpmock::prelude::*;
use serde_json::json;

const REPO: &str = "tier4/carla-packs";
const TOKEN: &str = "gho_user_token";
const DRIVE: &str = "0ABcdEfGhIjKlUk9PVA";
const KEY: &str = "MiniPack-1.0.0-carla-0.10.0-Linux.tar.gz";

fn rclone_binary() -> Option<PathBuf> {
    let bin = std::env::var_os("CARLA_PACK_TEST_RCLONE").map(PathBuf::from).unwrap_or_else(|| "rclone".into());
    let ok = Command::new(&bin).arg("version").output().is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipped: rclone is not installed (set CARLA_PACK_TEST_RCLONE)");
    }
    ok.then_some(bin)
}

struct World {
    github: MockServer,
    dir: tempfile::TempDir,
    binary: PathBuf,
}

impl World {
    fn new() -> Option<Self> {
        let binary = rclone_binary()?;
        let w = Self { github: MockServer::start(), dir: tempfile::tempdir().unwrap(), binary };
        fs::create_dir_all(w.drive().join("packs")).unwrap();
        Some(w)
    }

    /// The folder the remote `gdrive` serves, standing in for the shared drive.
    fn drive(&self) -> PathBuf {
        self.dir.path().join("drive")
    }

    fn rclone(&self, remote: &str) -> Rclone {
        let var = |k: &str, v: &Path| (k.into(), v.as_os_str().to_owned());
        Rclone {
            binary: self.binary.clone(),
            remote: remote.into(),
            service_account_file: None,
            env: vec![
                var("RCLONE_CONFIG", &self.dir.path().join("rclone.conf")),
                ("RCLONE_CONFIG_GDRIVE_TYPE".into(), "alias".into()),
                var("RCLONE_CONFIG_GDRIVE_REMOTE", &self.drive()),
            ],
        }
    }

    fn client(&self, remote: &str) -> Client {
        Client::with_options(ClientOptions {
            token: Some(Token { value: TOKEN.into(), source: "test" }),
            repos: vec![REPO.into()],
            github_api: self.github.base_url(),
            cache_dir: self.dir.path().join("cache"),
            rclone: self.rclone(remote),
        })
        .unwrap()
    }

    fn storage_text() -> String {
        format!(
            "schema = 1\n\n[storage]\ntype = \"rclone\"\nbackend = \"drive\"\nteam_drive = \"{DRIVE}\"\nprefix = \"packs/\"\n"
        )
    }

    fn serve(&self, body: &[u8]) {
        let text = format!(
            "{}\n[[pack]]\nname = \"MiniPack\"\nversion = \"1.0.0\"\nbase_release = \"carla-0.10.0-Linux\"\n\
             platform = \"Linux\"\nkey = \"{KEY}\"\nsha256 = \"{}\"\nsize = {}\nmaps = [\"MiniTown\"]\n",
            Self::storage_text(),
            hex_sha256(body),
            body.len()
        );
        let path = format!("/repos/{REPO}/contents/carla-packs.toml");
        self.github.mock(|when, then| {
            when.method(GET).path(&path).header("authorization", format!("Bearer {TOKEN}"));
            then.status(200).body(&text);
        });
    }
}

#[test]
fn packs_come_from_the_drive_through_rclone() {
    let Some(w) = World::new() else { return };
    let body = mini_pack(&[]);
    fs::write(w.drive().join("packs").join(KEY), &body).unwrap();
    w.serve(&body);

    let c = w.client("gdrive");
    let release = c.find_map("MiniTown", None, &Filter::default()).unwrap();
    let pack = c.install(&release, &w.dir.path().join("Packs"), &InstallOptions::default()).unwrap();
    assert_eq!(pack.maps, ["MiniTown"]);
    assert_eq!(fs::read(c.cached_path(&release.pack.sha256)).unwrap(), body);

    let status = c.credentials_status(REPO).unwrap();
    assert_eq!(status.storage, format!("Google Drive shared drive {DRIVE}"));
    assert_eq!(status.rclone.as_deref(), Some("rclone remote gdrive"));
    assert_eq!(json!(status.mode), json!("rclone"));
}

#[test]
fn access_problems_have_their_own_codes() {
    let Some(w) = World::new() else { return };
    let body = mini_pack(&[]);
    w.serve(&body);
    let fetch = |c: Client| {
        let release = c.find_map("MiniTown", None, &Filter::default()).unwrap();
        c.fetch(&release).unwrap_err()
    };

    // Not on the drive (or not shared with the account: Google Drive answers alike).
    let e = fetch(w.client("gdrive"));
    assert_eq!(e.code(), "storage_no_access", "{e}");
    assert!(e.to_string().contains("rclone remote gdrive"), "{e}");

    // No such remote: `carla-pack drive login` has not been run.
    let e = fetch(w.client("carla-pack-gdrive"));
    assert_eq!(e.code(), "storage_unauthenticated", "{e}");
    assert!(e.to_string().contains("carla-pack drive login"), "{e}");

    // No rclone at all.
    let mut c = w.client("gdrive");
    let mut r = w.rclone("gdrive");
    r.binary = w.dir.path().join("no-rclone-here");
    c = Client::with_options(ClientOptions {
        token: Some(Token { value: TOKEN.into(), source: "test" }),
        repos: c.repos().to_vec(),
        github_api: w.github.base_url(),
        cache_dir: c.cache_dir().to_path_buf(),
        rclone: r,
    })
    .unwrap();
    let e = fetch(c);
    assert_eq!(e.code(), "rclone_unavailable", "{e}");

    // A file that differs from the manifest is refused before it is downloaded (size), and
    // one of the right size by the SHA-256 the storage reports.
    let mut other = body.clone();
    *other.last_mut().unwrap() ^= 1;
    fs::write(w.drive().join("packs").join(KEY), &other).unwrap();
    let e = fetch(w.client("gdrive"));
    assert!(matches!(e, Error::Integrity(_)), "{e}");
    assert!(e.to_string().contains("nothing was downloaded"), "{e}");
    assert!(!w.dir.path().join("cache").join("blobs").read_dir().is_ok_and(|mut d| d.next().is_some()));
}

#[test]
fn publish_uploads_through_rclone_and_checks_what_arrived() {
    let Some(w) = World::new() else { return };
    let manifest = w.dir.path().join("carla-packs.toml");
    fs::write(&manifest, World::storage_text()).unwrap();
    let tarball = w.dir.path().join("MiniPack-2.0.0.tar.gz");
    let body = mini_pack_version("2.0.0", &[]);
    fs::write(&tarball, &body).unwrap();
    let opts = PublishOptions { upload: true, rclone: Some(w.rclone("gdrive")), ..Default::default() };

    let entry = publish(&manifest, &tarball, &opts, None).unwrap();
    assert_eq!(fs::read(w.drive().join("packs/MiniPack-2.0.0.tar.gz")).unwrap(), body);
    assert_eq!(entry.sha256, hex_sha256(&body));
    assert!(fs::read_to_string(&manifest).unwrap().contains("MiniPack-2.0.0.tar.gz"));

    // A file already there is not replaced without overwrite (checked before uploading).
    let again = fs::read_to_string(&manifest).unwrap();
    fs::write(&manifest, World::storage_text()).unwrap();
    match publish(&manifest, &tarball, &opts, None) {
        Err(Error::Usage(msg)) => assert!(msg.contains("already exists"), "{msg}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read_to_string(&manifest).unwrap(), World::storage_text());
    let replaced = publish(&manifest, &tarball, &PublishOptions { overwrite: true, ..opts.clone() }, None).unwrap();
    assert_eq!(replaced, entry);
    assert_eq!(fs::read_to_string(&manifest).unwrap(), again);
}

/// rclone stopping partway (a dropped connection) is resumed with `cat --offset`, and the
/// whole file is still checked against the manifest.
#[cfg(unix)]
#[test]
fn a_broken_download_resumes_from_where_it_stopped() {
    use std::os::unix::fs::PermissionsExt;
    let Some(w) = World::new() else { return };
    let body = big_pack(256 * 1024);
    fs::write(w.drive().join("packs").join(KEY), &body).unwrap();
    w.serve(&body);
    // The first `cat` (no --offset) prints 1000 bytes and fails; later ones are rclone's.
    let wrapper = w.dir.path().join("flaky-rclone");
    let log = w.dir.path().join("calls.log");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{log}'\nif [ \"$1\" = cat ] && [ \"$2\" != --offset ]; then\n  \
             '{bin}' \"$@\" | head -c 1000\n  echo 'connection reset' >&2\n  exit 1\nfi\nexec '{bin}' \"$@\"\n",
            log = log.display(),
            bin = w.binary.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let mut rclone = w.rclone("gdrive");
    rclone.binary = wrapper;
    let c = Client::with_options(ClientOptions {
        token: Some(Token { value: TOKEN.into(), source: "test" }),
        repos: vec![REPO.into()],
        github_api: w.github.base_url(),
        cache_dir: w.dir.path().join("cache"),
        rclone,
    })
    .unwrap();

    let release = c.find_map("MiniTown", None, &Filter::default()).unwrap();
    assert_eq!(fs::read(c.fetch(&release).unwrap()).unwrap(), body);
    let calls = fs::read_to_string(&log).unwrap();
    assert!(calls.lines().any(|l| l.starts_with("cat --offset 1000 -- gdrive,")), "{calls}");
}
