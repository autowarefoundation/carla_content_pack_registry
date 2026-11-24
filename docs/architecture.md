# Architecture

## Idea

The decision "may this person download this pack?" has moved from our own server to **GitHub repository permissions**.

- Can read the pack repository (GitHub, private) ⇔ can fetch its packs.
- Everything needed for that (where each pack is, its SHA-256, the read key) lives inside the repository.
- The client reads it with the user's own GitHub token and downloads directly from storage.

There is no central server, database or secret store.

## Flow

```
User's carla-pack / Python                    GitHub                          S3-compatible storage
──────────────────────────                    ──────                          ─────────────────────
1. GET /repos/{o}/{r}/contents/carla-packs.toml ─► 404 if not readable (stop)
   ← manifest (storage and [[pack]])
2. From a reference (OWNER/REPO/PACK@VER) or a map name,
   settle on one release (newest first; ambiguity is an error)
3. GET /repos/{o}/{r}/releases/tags/carla-pack-credentials
   GET .../releases/assets/{id}  ─► 302 (signed URL on another host; the token is not sent)
   ← credentials.json (read-only, time-limited)
4. GET {endpoint}/{bucket}/{prefix}{key}  (SigV4, UNSIGNED-PAYLOAD) ────────────────────► checked against the key's permissions
   ← tarball (saved while streaming SHA-256; on a disconnect, continued with Range: bytes=N-)
5. Check size and SHA-256 against the manifest → cache (blobs/<sha256>.tar.gz)
6. Extract, check every file against carla-pack.json, and confirm name, version, base release and platform match the manifest
7. Install into <packs_dir>/<name> → (optional) carla.Client.mount_content_pack
```

With Google Drive storage (`type = "rclone"`), steps 3 and 4 are replaced by rclone, run as a child process with the
user's own Google credentials (their rclone remote, or a service account in CI); nothing is vended through GitHub:

```
3'. rclone lsjson --stat --hash-type sha256 -- '<remote>,team_drive=…,root_folder_id=…,shared_with_me=false:<prefix><key>'
    ← size and Drive's SHA-256: refused before downloading if either differs from the manifest; a drive not shared
      with the account → storage_no_access, no remote / a rejected token → storage_unauthenticated
4'. rclone cat [--offset N] -- '<same>'   ← tarball on stdout (saved while streaming SHA-256; resumed with --offset)
```

The drive and folder are always set from the manifest (empty when absent), so a remote configured for another drive cannot redirect the read.

The key issuer (the pack repository's GitHub Actions, every 6 hours):

```
R2:  Cloudflare API (API token in a secret) → POST /accounts/{id}/r2/temp-access-credentials
     (object-read-only, limited to the prefix, 12 hours)
AWS: GitHub OIDC → sts:AssumeRoleWithWebIdentity (12 hours, limited to s3:GetObject and the prefix by a session policy)
→ replace credentials.json as an asset of the Release "carla-pack-credentials" (aborts unless the repository is private)
```

## Formats

### `carla-packs.toml` (root of the pack repository)

```toml
schema = 1

[storage]
endpoint = "https://<account>.r2.cloudflarestorage.com"   # https only (http only for localhost). An origin without a path
region = "auto"                                          # default auto (R2). For S3, the bucket's region
bucket = "carla-packs"
prefix = "packs/"                                        # optional. A relative path ending in "/"
addressing = "path"                                      # path (default) | virtual
credentials = "github-release"                           # github-release | anonymous (public bucket)
credentials_tag = "carla-pack-credentials"               # optional
# type = "s3" may be omitted (default). For public packs on Hugging Face:
# type = "huggingface" / repo = "OWNER/NAME" / repo_type = "dataset" | "model" / revision = "main" / prefix
# For a Google Drive shared drive read through rclone (each user's Google account):
# type = "rclone" / backend = "drive" / team_drive = "<shared drive ID>" / root_folder_id = "<folder ID>" (optional) / prefix

[[pack]]
name = "NishishinjukuPack"            # CARLA pack name (letters, digits and _, starting with a letter)
version = "1.2.0"
base_release = "carla-0.10.0-Linux"
platform = "Linux"
key = "NishishinjukuPack-1.2.0-carla-0.10.0-Linux.tar.gz"   # key relative to the prefix
sha256 = "…"                          # SHA-256 of the tarball (64 lowercase hex digits)
size = 3221225472
maps = ["NishishinjukuMap"]
description = "…"                     # optional
yanked = false                        # optional. true means it is not selected (still fetchable from a lock pinned by sha256)

[pack.formats]                        # optional. Map data formats per map (written by publish; see "Map data formats" below)
NishishinjukuMap = ["autoware", "lanelet2", "navmesh", "opendrive", "sumo"]
```

- Unknown keys are an error (so typos are not missed).
- A duplicate `(name, version, base_release, platform)` is an error. `carla-pack publish` only appends; it never replaces an existing entry.
- Selection: the latest version among the entries matching the conditions (name, version, base release, platform, sha256). If the latest version exists for several base releases / platforms, it is ambiguous and an error.
  Versions are compared numerically per separator (`.` `-` `+` `_`), and `1.0.0` counts as newer than `1.0.0-rc1`.
- A map name given as a path such as `/Game/.../Town01` is matched by its last component.
- `formats` lists format names per map name. `find --format` searches this index, and `find --assets` also covers map names that appear only here (data-only maps without a CARLA level).
  Releases published before `formats` was written are not found by format, only by map name (`carla-pack formats` still works on them once fetched).
  A manifest containing `formats` cannot be read by an older carla-pack that does not know this key (from before `formats` was added).

### Map data formats

A pack's map data can be looked up by map name and format name. No CARLA server is needed.

- **Standard formats**: every pack has them, built from each map's `xodr` / `nav` / `tm` / `map_logic` in `carla-pack.json`. The format names are `opendrive` / `navmesh` / `traffic_manager` (a folder) / `map_logic`.
- **Asset file (optional)**: if a pack carries `Content/Config/roadgen_asset.json`, its formats are added. roadgen's `carla_pack_assets` writes this file; it records that Lanelet2 and a point cloud map (Autoware), SUMO, CommonRoad, ClipGT, RDS-HQ, GPUDrive, WOMD, etc. were put in `Content/HDMaps/<map>/`. A pack without it has only the standard formats. When both name a format, the standard one is used.

```json
{
  "schema": 1,
  "generator": {"name": "roadgen", "version": "0.43.0"},
  "maps": {
    "Highway": {
      "formats": {
        "lanelet2": {"type": "file", "path": "Content/HDMaps/Highway/autoware/lanelet2_map.osm"},
        "autoware": {"type": "directory", "path": "Content/HDMaps/Highway/autoware",
                     "files": {"lanelet2_map": "…/lanelet2_map.osm", "pointcloud_map": "…/pointcloud_map.pcd",
                               "map_projector_info": "…/map_projector_info.yaml"}}
      }
    }
  }
}
```

- Every path is a normalized path relative to the pack folder (the folder holding `carla-pack.json`); a path that leaves it is an error.
- In a built pack, every file the asset file points to must be listed in `files` of `carla-pack.json` (confirming it is covered by the SHA-256 check at install). Otherwise the whole asset file is refused.
- A `schema` other than 1 is an error.

### `credentials.json` (Release asset)

```json
{
  "version": 1,
  "access_key_id": "…",
  "secret_access_key": "…",
  "session_token": "…",
  "expires_at": "2026-11-03T00:00:00Z"
}
```

`session_token` and `expires_at` are optional (omitted for a long-lived key). `expires_at` is RFC 3339 (an offset is allowed).
A key with less than 2 minutes left is not used; the error says the key workflow is not running.

## Code

| File | Role |
| --- | --- |
| `src/github.rs` | Access check: reads the manifest and key from the GitHub API. 404 → `no_access` |
| `src/manifest.rs` | Types and validation of `carla-packs.toml`, the reference `OWNER/REPO/PACK[@VER]`, release selection |
| `src/sigv4.rs` | AWS Signature V4 (header signing, `UNSIGNED-PAYLOAD`). Verified with AWS's official test vectors |
| `src/hf.rs` | Fetching from public Hugging Face repositories (anonymous `resolve` URLs, resumed with Range) |
| `src/rclone.rs` | Google Drive through rclone: the pinned remote spec, `lsjson` (access, size, SHA-256), `cat --offset` (streamed, resumed), `copyto` + check (publish), rclone errors → codes |
| `src/s3.rs` | GET (resumed with Range) / HEAD / PUT; multipart upload above 64 MiB (aborted on failure). Path / virtual-host addressing |
| `src/client.rs` | `resolve` / `find_map` / `fetch` (cache and verification) / `install` / `unpack` / `formats` / `map_data` / `publish` |
| `src/pack.rs` | Safe tarball extraction, checking every file against `carla-pack.json`, installation |
| `src/formats.rs` | Map data formats: reading the standard formats from `carla-pack.json` and the formats from `roadgen_asset.json`, the index, path resolution |
| `src/store.rs` | Token lookup, saved settings (login, repositories to search), cache location |
| `src/auth.rs` | GitHub device flow (client ID only, no client secret) |
| `src/python.rs` | PyO3 bindings (`carla_pack_registry._core`) |
| `python/carla_pack_registry/deploy.py` | `deploy_pack` / `ensure_map`: mount / unmount on the CARLA server |
| `python/carla_pack_registry/publish.py` | `publish`: check (Rust `plan_publish`) → upload (S3 and rclone in Rust, Hugging Face via `huggingface_hub`) → append (`append_entry`) |
| `python/carla_pack_registry/cli.py` | The `carla-pack` command |
| `templates/pack-repo/` | Manifest template and key rotation workflows for pack repositories |

## Differences from the old Registry server (Workers / Lambda)

| | Old: Registry server | New: GitHub + direct S3 |
| --- | --- | --- |
| Access decision | The server looked up GitHub org / team membership and evaluated an ACL | GitHub decides by Read permission on the repository (404 if not readable) |
| Unit of sharing | Per-bucket ACL (user / org / team) | Per repository (and bucket prefix). To share with a team, add the team to the repository |
| Download | 5-minute URL signed by the server | The user signs with SigV4 using a 12-hour read-only key |
| Secrets | The server held every bucket's key (encrypted) and an IAM key for STS | None centrally. The key sources (Cloudflare API token / OIDC role) exist only in each pack repository's Actions |
| Listing | Live listing of the bucket | Only what was explicitly published in the manifest (pinned by SHA-256, immutable) |
| Operations | Workers + D1 or Lambda + DynamoDB, WAF, Terraform | None (only each pack repository's scheduled workflow) |
| Auditing | The server's audit log | GitHub's permission change history and the storage access log (per key ID) |
