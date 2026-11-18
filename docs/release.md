# Releases (PyPI)

The Python package `carla-pack-registry` (command `carla-pack`, module `carla_pack_registry`) is released by PR labels (the same mechanism as [simple_lanelet2](https://github.com/hakuturu583/simple_lanelet2), `.github/workflows/release.yml`).

| Label | When merged into main |
| --- | --- |
| `release:patch` | 0.2.0 → 0.2.1 |
| `release:minor` | 0.2.0 → 0.3.0 |
| `release:major` | 0.2.0 → 1.0.0 |
| none | nothing happens |

Add exactly one label (with more than one, the release job fails). Labelled PRs merged in quick succession each bump from the latest main, so no release is skipped and no two claim the same version. On merge, the following run in order within the same workflow run:

1. `scripts/bump-cli-version.py` bumps the version in `Cargo.toml` and `Cargo.lock`, commits `chore(release): vX.Y.Z` to main, and creates the `vX.Y.Z` tag and a GitHub Release (generated notes)
2. fmt / clippy / `cargo test` run again on the tagged commit, and the tag is checked against the package version
3. Wheels (abi3, Python 3.10 and later) for Linux x86_64 / aarch64 (manylinux 2_28), macOS arm64 / x86_64 and Windows x64 are built on runners of each architecture, installed into a clean venv and tested. The sdist is built and installed once to check it
4. After `twine check --strict`, upload to PyPI with Trusted Publishing (`skip-existing`; anything that failed can be added by re-running)

The wheel tests run without extras, so the E2E tests using moto and the Hugging Face tests are skipped. `ci.yml` runs them on every PR.

The single definition of the version is `[package] version` in `Cargo.toml` (`pyproject.toml` is dynamic). Do not change it by hand; use a label or `scripts/bump-cli-version.py`.

## One-time setup

1. On PyPI (https://pypi.org/manage/account/publishing/), add a **pending publisher**: PyPI project name `carla-pack-registry`, owner `autowarefoundation`, repository `carla_content_pack_registry`, workflow `release.yml`, environment `pypi`.
2. Create `pypi` under GitHub's Settings → Environments (adding Required reviewers, if you like, puts an approval before publishing).

If a PR with a release label is merged before this is set up, the tag and GitHub Release are still created and only the publish job fails. The wheels and sdist stay as artifacts of that run, so after the setup they can be published with "Re-run failed jobs".

## Other entry points

- **Trial build**: Actions → Release → Run workflow (`workflow_dispatch`). It only builds and tests the wheels and sdist; nothing is tagged or published. PRs that change `release.yml`, `scripts/bump-cli-version.py`, `Cargo.*`, `pyproject.toml`, `README.md` or `LICENSE` run the same dry run.
- **Cutting a tag by hand (a hotfix, etc.)**: bump the version with `scripts/bump-cli-version.py X.Y.Z` and commit, then `git tag -a vX.Y.Z && git push origin vX.Y.Z`. Pushing the tag runs only the build and publish (it stops if the tag and the package version differ).
