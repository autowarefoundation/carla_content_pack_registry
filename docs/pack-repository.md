# Creating a pack repository

A pack repository is a GitHub repository that defines who can fetch which packs.
Only people with Read permission can fetch its contents (the list of packs and the temporary key for reading the storage).

Use one repository per audience (e.g. `tier4/carla-packs` for the whole company, `tier4/carla-packs-restricted` for a specific team).
They can share a bucket, but **use separate prefixes** (keys are issued per prefix).

## 1. Repository

1. Create a **private** repository (on a public repository the key workflow fails).
2. Copy [templates/pack-repo/carla-packs.toml](../templates/pack-repo/carla-packs.toml) to its root and fill in `[storage]`.
3. Give **Read** to the users / teams / organizations who should receive the packs. Give **Write** only to those who publish packs.
4. Recommended: protect the default branch and require review for changes to `carla-packs.toml` (CODEOWNERS).

## 2. Storage and keys

### Cloudflare R2

1. Create a bucket (public access, r2.dev and custom domains **disabled**).
2. Create an **R2 API token (the parent)**: Object Read on the bucket. Note the Access Key ID it shows (the Secret is not needed).
   Temporary keys are made from this parent and never exceed its permissions.
3. Create a **Cloudflare API token** (with permission to create R2 temporary credentials).
4. In the repository's Settings → Secrets and variables → Actions:
   - secret `CLOUDFLARE_API_TOKEN`
   - variables `R2_ACCOUNT_ID`, `R2_BUCKET`, `R2_PREFIX` (e.g. `packs/`), `R2_PARENT_ACCESS_KEY_ID`
5. Copy [carla-pack-credentials-r2.yml](../templates/pack-repo/.github/workflows/carla-pack-credentials-r2.yml) to `.github/workflows/` and run it once manually from the Actions tab.

`carla-packs.toml`:

```toml
[storage]
endpoint = "https://<ACCOUNT_ID>.r2.cloudflarestorage.com"
region = "auto"
bucket = "<bucket>"
prefix = "packs/"
credentials = "github-release"
```

### AWS S3

1. Create a bucket (Block Public Access enabled).
2. Register GitHub's OIDC provider (`token.actions.githubusercontent.com`) in IAM (if not already).
3. Create an IAM role:
   - Trust policy: `sts:AssumeRoleWithWebIdentity` for `token.actions.githubusercontent.com`,
     with conditions `token.actions.githubusercontent.com:sub` = `repo:<OWNER>/<REPO>:ref:refs/heads/<default branch>` and `aud` = `sts.amazonaws.com`
   - Permissions: `s3:GetObject` on the bucket
   - Maximum session duration: 12 hours
4. Variables `AWS_ROLE_ARN`, `AWS_REGION`, `S3_BUCKET`, `S3_PREFIX`.
5. Copy [carla-pack-credentials-aws.yml](../templates/pack-repo/.github/workflows/carla-pack-credentials-aws.yml) and run it manually.
   The session is further narrowed to `s3:GetObject` and the prefix by a session policy.

`carla-packs.toml`:

```toml
[storage]
endpoint = "https://s3.<region>.amazonaws.com"
region = "<region>"
bucket = "<bucket>"
prefix = "packs/"
credentials = "github-release"
```

### Other S3-compatible storage (MinIO, SeaweedFS, etc.)

If no API for issuing temporary keys is available, place a **read-only key limited to the bucket** by hand (it does not expire; recreate the key when you remove someone).

```bash
cat > credentials.json <<'EOF'
{"version": 1, "access_key_id": "...", "secret_access_key": "..."}
EOF
gh release create carla-pack-credentials --prerelease --title "carla-pack storage credentials" --notes "Do not edit." || true
gh release upload carla-pack-credentials credentials.json --clobber
rm credentials.json
```

### Hugging Face (for public packs)

Packs that anyone may have can be placed in a public Hugging Face Hub repository (a dataset is recommended).
No storage, key or key workflow is needed, and they can sit alongside the Autoware maps (`AutowareFoundation/carla-ue5-maps`).
Downloads use anonymous `resolve` URLs and send no token. The contents are verified against the manifest's SHA-256.

1. Create a public repository on Hugging Face (not gated).
2. Copy [templates/pack-repo/carla-packs.huggingface.toml](../templates/pack-repo/carla-packs.huggingface.toml) as `carla-packs.toml`. The pack repository on GitHub can be public too.
3. Run `carla-pack publish` to upload and append to the manifest. The upload to the Hub is done by Hugging Face's official `huggingface_hub` (large files go through as is, with the Hub's transfer method):

```bash
pip install 'carla-pack-registry[huggingface]'
hf auth login            # or HF_TOKEN (a token with write access to that repository)
carla-pack publish NishishinjukuPack-1.2.0-carla-0.10.0-Linux.tar.gz
git commit -am "NishishinjukuPack 1.2.0" && git push
```

   Everything is checked before uploading (a duplicate of the same release, an existing file at the same path; the latter is replaced with `--overwrite`).
   If the upload fails, nothing is appended to the manifest. `revision` must be a branch (a commit cannot be written to).

```toml
[storage]
type = "huggingface"
repo = "AutowareFoundation/carla-packs"
repo_type = "dataset"     # default. model also works
revision = "main"         # default. A branch, tag or commit
prefix = "packs/"
```

This cannot be used for private packs (they are read anonymously). Put private packs on R2 / S3.

### Google Drive (shared drive, read through rclone)

For a Google Workspace: the packs sit on a **shared drive**, and **who may download them is the drive's sharing**
(a Google group as Viewer, say). Each user reads it with their own Google account through [rclone](https://rclone.org/),
so there is no storage key, no key workflow and nothing to rotate. Removing someone from the group stops their downloads at once.

```
carla-pack ──(1) GitHub API: carla-packs.toml (private repository) ──► GitHub
           ──(2) rclone cat, as the user's Google account ──────────► Google Drive shared drive
           ──(3) SHA-256 and carla-pack.json checked → install → mount
```

1. Create a **shared drive** (not a folder in someone's My Drive: its files would belong to that person, and a
   service account cannot own files at all). Share it with the Google group of pack users as **Viewer**,
   and with the publishers as **Content manager**.
2. Create an **OAuth client** for your Workspace (Google Cloud console → APIs & Services, with the Google Drive API enabled):
   an *Internal* consent screen (no Google review is needed for an internal app, even with Drive scopes),
   and a client of type *Desktop app*. Hand its client ID and secret to your users (for an internal desktop
   client the secret is not confidential), e.g. as `CARLA_PACK_DRIVE_CLIENT_ID` / `CARLA_PACK_DRIVE_CLIENT_SECRET`.
   rclone's built-in client works too, but it is shared by every rclone user in the world and quickly rate-limited.
3. Copy [templates/pack-repo/carla-packs.rclone.toml](../templates/pack-repo/carla-packs.rclone.toml) as `carla-packs.toml`
   and set `team_drive` to the shared drive's ID (the last part of its `drive.google.com/drive/folders/...` URL).
   Keep the pack repository **private** and give Read to the same people, so that only they see the list of packs.

`carla-packs.toml`:

```toml
[storage]
type = "rclone"
backend = "drive"
team_drive = "<shared drive ID>"
# root_folder_id = "<folder ID>"   # optional: a folder in the shared drive (or a folder shared with the users)
prefix = "packs/"
```

Each user, once (a current rclone on PATH, or in `CARLA_PACK_RCLONE`):

```bash
carla-pack drive login            # creates the rclone remote carla-pack-gdrive (read-only scope), in the browser
carla-pack credentials tier4/carla-packs   # checks that the drive can be read
```

- Without a browser (a server): run `rclone authorize drive <client_id> <client_secret>` on a machine with one,
  and pass what it prints to `carla-pack drive login --token-stdin` (or use `--headless` and paste it when rclone asks).
- An existing Google Drive remote can be used instead: `CARLA_PACK_RCLONE_REMOTE=<name>`. carla-pack sets the drive and
  folder on every call (rclone's `remote,team_drive=...,root_folder_id=...:path`), so the remote's own settings do not matter.
- **CI / unattended runs**: create a service account, add it to the shared drive as Viewer, and set
  `CARLA_PACK_RCLONE_SERVICE_ACCOUNT_FILE` to its JSON key (no rclone remote is needed then). Keep the key in the CI's secrets.

Publishing uploads with rclone (`copyto`, which compares the uploaded file's hash with the tarball's) before the entry
is appended; an existing file is not replaced without `--overwrite`. It needs write access: a remote of its own, or a service account that is a Content manager
of the drive (`CARLA_PACK_RCLONE_SERVICE_ACCOUNT_FILE`):

```bash
carla-pack drive login --write --remote carla-pack-gdrive-rw
CARLA_PACK_RCLONE_REMOTE=carla-pack-gdrive-rw carla-pack publish NishishinjukuPack-1.2.0-carla-0.10.0-Linux.tar.gz
git commit -am "NishishinjukuPack 1.2.0" && git push
```

Limits: Google Drive is not a CDN. Google does not publish its download limits, and many machines fetching the same
multi-GB file at once may get `downloadQuotaExceeded` (reported as `storage_error`; retry later). carla-pack fetches
each release once into its cache, so share the cache (`CARLA_PACK_CACHE_DIR`) between jobs on the same machine.

### Public bucket

Packs that anyone may fetch can be placed in a publicly readable bucket with `credentials = "anonymous"` (no key needed; the repository can be public as well).

## 3. Publishing a pack

Upload a tarball built with CARLA's `Util/ContentPacks/carla_pack.py build` using a write key, and append it to the manifest.

```bash
git clone git@github.com:tier4/carla-packs.git && cd carla-packs
export CARLA_PACK_UPLOAD_ACCESS_KEY_ID=...        # a key that can write (AWS_ACCESS_KEY_ID etc. also work)
export CARLA_PACK_UPLOAD_SECRET_ACCESS_KEY=...
carla-pack publish ~/build/NishishinjukuPack-1.2.0-carla-0.10.0-Linux.tar.gz
git commit -am "NishishinjukuPack 1.2.0" && git push            # (a PR if the branch is protected)
```

- Name, version, base release, platform and maps are read from `carla-pack.json`, the SHA-256 and size are computed, and a `[[pack]]` is appended (existing comments are kept).
- If the same `(name, version, base_release, platform)` already exists, it is refused. **Do not change a published release** (to change its contents, bump the version).
- Files over 64 MiB are sent as a multipart upload (64 MiB parts by default, changed with `--part-size-mib`). Packs of several GB to hundreds of GB can be published as is (the limit is S3 / R2's 10,000 parts, and the part size is raised automatically). Each part is retried on failure; if it still fails, the upload is aborted and nothing is appended to the manifest.
- If the file was already uploaded with another tool (`aws s3 cp` / `rclone`, etc.), `--no-upload` only appends.
- On Google Drive storage the upload goes through rclone with the remote of `carla-pack drive login --write` (or a service account); see above.
- To withdraw a release, add `yanked = true` to its entry (it can still be fetched from a lock pinned by SHA-256).

## 4. Checking

```bash
carla-pack credentials tier4/carla-packs     # key ID and expiry (the secret is not shown); for Google Drive, that the drive can be read
carla-pack ls tier4/carla-packs
carla-pack pull tier4/carla-packs/NishishinjukuPack -o /tmp/check
```
