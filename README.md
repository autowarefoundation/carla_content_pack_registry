# carla-pack

A client for **distributing CARLA content packs through GitHub repository permissions**. There is no server.

```
carla-pack ──(1) GitHub API: read carla-packs.toml ──────────────► GitHub (private repository)
           ──(2) GitHub API: read the read-only S3 key ───────────►   └ Release "carla-pack-credentials"
           ──(3) download directly with SigV4 ────────────────────► S3-compatible storage (R2 / S3 / MinIO …)
           ──(4) verify with SHA-256 and carla-pack.json → install → mount ─► CARLA server
```

- **GitHub checks access.** Only users who can read the (private) pack repository can fetch
  the manifest `carla-packs.toml` and the read-only storage key stored in a Release. Anyone else just gets a 404 from GitHub.
  Who receives the packs is decided directly by Read permission on that repository (users / teams / organizations).
- **Files are downloaded directly from S3-compatible storage** (Cloudflare R2, AWS S3, MinIO, SeaweedFS, etc.).
  Packs that anyone may have can also live in a **public Hugging Face Hub repository** (no key, anonymous download).
- Inside a Google Workspace, packs can also live on a **Google Drive shared drive**, read through [rclone](https://rclone.org/)
  with each user's own Google account: the drive's sharing decides who may download them, on top of the (private) pack repository
  ([docs/pack-repository.md](docs/pack-repository.md#google-drive-shared-drive-read-through-rclone)).
- The key is a **temporary key valid for 12 hours, recreated every 6 hours** by the pack repository's GitHub Actions. It can only read the pack prefix of the bucket.
  R2 issues it through its temporary credentials API and AWS through GitHub OIDC, so no long-lived key is stored anywhere.
- Downloaded files are verified against the manifest's **SHA-256 and size**, and then against the hashes of every file in the pack's own `carla-pack.json`, before they are installed.
- The core (GitHub API, SigV4, download, verification, extraction) is Rust; the CLI and mounting into CARLA are Python (PyO3 / maturin, abi3 wheel).

The previous version (a Registry server on Cloudflare Workers / AWS Lambda) has been retired. How this design differs from it is described in [docs/architecture.md](docs/architecture.md).

## Using packs (downloading)

```bash
pip install carla-pack-registry   # during development, see "Development"

gh auth login                     # the GitHub CLI's token is used as is (GH_TOKEN / GITHUB_TOKEN / carla-pack login also work)
carla-pack whoami
carla-pack repo add tier4/carla-packs     # repositories searched by map name (CARLA_PACK_REPOS also works)

carla-pack ls                             # packs in the repositories you can read
carla-pack find NishishinjukuMap          # packs containing that map
carla-pack install NishishinjukuMap --map \
  --packs-dir /opt/carla/Linux/CarlaUnreal/Packs --mount localhost:2000
carla-pack install tier4/carla-packs/NishishinjukuPack@1.2.0 --packs-dir ./Packs   # by reference
carla-pack pull tier4/carla-packs/NishishinjukuPack -o ./downloads               # only fetch the tarball
```

The PyPI distribution is `carla-pack-registry`, the Python module is `carla_pack_registry` and the command is `carla-pack`
(it does not clash with CARLA's own `Util/ContentPacks/carla_pack.py` in the same environment).

A pack is referred to as `OWNER/REPO/PACK[@VERSION]`. Without a version the latest is used (narrowed with `--base-release` / `--platform`);
`--sha256` pins it to that artifact (for lock files).

### From Python (in a scenario library)

```python
import typesafe_carla.carla as carla   # the official carla module also works
from carla_pack_registry import Client, deploy_pack, ensure_map

client = carla.Client("localhost", 2000)

# Only when the server lacks the map: find it in the readable repositories, fetch, verify, install and mount it. Otherwise do nothing (None).
pack = ensure_map(client, "NishishinjukuMap", "/opt/carla/Linux/CarlaUnreal/Packs",
                  repos=["tier4/carla-packs"])
client.load_world("NishishinjukuMap")

# Reproducible fetch from a lock file's contents (reference + SHA-256)
deploy_pack("tier4/carla-packs/NishishinjukuPack", packs_dir, carla_client=client,
            sha256=pack["sha256"])

# Low-level API
c = Client()                                     # token and searched repositories come from the environment / saved settings
release = c.find_map("NishishinjukuMap")         # {"ref", "repo", "name", "version", "sha256", "maps", ...}
path = c.fetch(release)                          # fetch into the cache (named by SHA-256), verify, return the path
c.install(release, packs_dir)                    # verify and install into <packs_dir>/<name> (no-op if identical)
```

- `packs_dir` must be readable by the CARLA server at the same path (the package's `Linux/CarlaUnreal/Packs`, `-carla-packs=<dir>`, or any folder if you mount it).
- If the same pack (name, version, base release, platform) is already installed, nothing is downloaded. Replacing another version needs `force=True` (a mounted pack is unmounted first).
- Downloads are cached in `~/.cache/carla-pack/blobs/<sha256>.tar.gz` (`CARLA_PACK_CACHE_DIR`).
  A multi-gigabyte pack uses **about twice its size** on disk: the cached tarball plus the extracted pack.
- If the connection drops, the download resumes from the received offset with a Range request (up to 5 times). A resumed download is still verified against the whole file's SHA-256.

### Getting map data by format

A pack's map data can be retrieved as local paths by map name and format name. No CARLA server is needed (the pack is fetched, verified and extracted into the cache).

- Every pack has the standard formats from `carla-pack.json` (`opendrive` / `navmesh` / `traffic_manager` / `map_logic`).
- If the pack carries `Content/Config/roadgen_asset.json` (optional), its formats too (written by roadgen's `carla_pack_assets`: `lanelet2` / `autoware` (Lanelet2, point cloud map, projector) / `pointcloud` / `sumo` / `commonroad` / `clipgt` / `rdshq` / `gpudrive` / `womd`, etc.).

```bash
carla-pack formats hakuturu583/carla-packs-test/RoadgenHighwayPack           # map × format table
carla-pack data hakuturu583/carla-packs-test/RoadgenHighwayPack --format lanelet2
carla-pack data RoadgenHighway --map --format autoware --file pointcloud_map  # one file inside a folder
carla-pack find RoadgenHighway --format lanelet2      # packs with that format (searched in the index written at publish)
carla-pack find SomeDataOnlyMap --assets              # also find maps that only the asset file names
carla-pack ls --formats                               # add a formats column to the list
carla-pack formats ./Packs/RoadgenHighwayPack         # an installed pack folder or a tarball also works
```

```python
from carla_pack_registry import Client, map_data, map_formats

client = Client()
release = client.find_map("RoadgenHighway", format="autoware")
autoware_dir = client.map_data(release, "RoadgenHighway", "autoware")   # usable directly as Autoware's map_path
pcd = client.map_data(release, "RoadgenHighway", "autoware", file="pointcloud_map")
client.formats(release)          # {map: {format: {type, path, files, origin}}}
map_formats("/opt/carla/CarlaUnreal/Packs/RoadgenHighwayPack")          # an installed pack
map_data("/opt/carla/CarlaUnreal/Packs/RoadgenHighwayPack", "RoadgenHighway", "opendrive")
```

`carla-pack publish` writes each map's formats into the entry in `carla-packs.toml` (`formats`). `find --format` searches this index, so it downloads nothing.

### Settings

| Item | How |
| --- | --- |
| GitHub token | in order: `CARLA_PACK_TOKEN` → `GH_TOKEN` → `GITHUB_TOKEN` → the one saved by `carla-pack login` → `gh auth token` |
| Repositories to search | `--repo` (repeatable), `CARLA_PACK_REPOS` (comma-separated), `carla-pack repo add` |
| Where settings are saved | `CARLA_PACK_CONFIG_DIR` (default `~/.config/carla-pack/config.json`, permissions 600) |
| GitHub Enterprise Server | `CARLA_PACK_GITHUB_API` (e.g. `https://ghe.example.com/api/v3`) |
| Google Drive storage (rclone) | `carla-pack drive login` creates the rclone remote `carla-pack-gdrive`; `CARLA_PACK_RCLONE` (the rclone executable), `CARLA_PACK_RCLONE_REMOTE` (another remote), `CARLA_PACK_RCLONE_SERVICE_ACCOUNT_FILE` (a service account key, for CI), `CARLA_PACK_DRIVE_CLIENT_ID` / `CARLA_PACK_DRIVE_CLIENT_SECRET` (your Workspace's OAuth client, for `drive login`) |

`carla-pack login` is a device flow with the client ID of a GitHub App (recommended: Contents read-only, installed on the pack repositories)
(`--client-id` / `CARLA_PACK_GITHUB_CLIENT_ID`; for an OAuth App, `--scope repo`). Not needed if you have run `gh auth login`.
In CI, pass a fine-grained PAT or a GitHub App token that can read the pack repositories in `CARLA_PACK_TOKEN`
(a workflow's `GITHUB_TOKEN` cannot read other repositories).

## Distributing packs (creating a pack repository)

See [docs/pack-repository.md](docs/pack-repository.md). In short:

1. Create a private repository and put [templates/pack-repo/carla-packs.toml](templates/pack-repo/carla-packs.toml) in it. Give Read permission to whoever should receive the packs.
2. Set up a bucket (R2 / S3) and an issuer for read-only keys, and copy the [key rotation workflow](templates/pack-repo/.github/workflows/).
3. Run `carla-pack publish <Pack>.tar.gz` to upload and append to the manifest, then commit and push.
   The upload goes to R2 / S3 (carla-pack sends it directly as a multipart upload) or to Hugging Face (`pip install 'carla-pack-registry[huggingface]'`, through `huggingface_hub`).

## Development

```bash
uv venv && . .venv/bin/activate
uv pip install "maturin>=1.9,<2" pytest "moto[server]==5.2.5" boto3 "huggingface_hub>=0.32"
maturin develop            # build the Rust core and install carla_pack_registry
cargo fmt --check && cargo clippy --all-targets --features python -- -D warnings
cargo test                 # Rust: the official SigV4 test vectors, every flow against mock GitHub / S3 servers, extraction safety
pytest                     # Python: the CARLA side (mount / unmount order) and E2E with moto (S3 with IAM auth) + a fake GitHub
maturin build --release    # wheel (abi3: one wheel for Python 3.10 and later)
```

PyPI releases are driven by PR labels. Merge a PR with exactly one `release:major|minor|patch` label and
`.github/workflows/release.yml` bumps the version, tags, creates the GitHub Release and publishes to PyPI ([docs/release.md](docs/release.md)).

## Documentation

| Document | Contents |
| --- | --- |
| [docs/architecture.md](docs/architecture.md) | How it works, the manifest and key file formats, differences from the old Registry server |
| [docs/security.md](docs/security.md) | Trust boundaries, revocation, residual risks |
| [docs/pack-repository.md](docs/pack-repository.md) | How to create a pack repository (R2 / AWS / other S3-compatible storage / Hugging Face / Google Drive) |
| [docs/release.md](docs/release.md) | How `carla-pack-registry` is released to PyPI (label-driven, Trusted Publishing) |

## License

[Apache License 2.0](LICENSE).
