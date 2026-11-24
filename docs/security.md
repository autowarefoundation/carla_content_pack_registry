# Security

## Trust boundaries

| What is trusted | Why |
| --- | --- |
| GitHub repository permissions | "Can read the pack repository = may fetch" is the definition. The manifest and key can only be read with this permission |
| People with write access to the pack repository | They can change the manifest (where to fetch from, with which SHA-256) and the key workflow. Limit repository Write permission to pack publishers |
| Permissions of the storage key | The temporary key is `object-read-only` (R2) / `s3:GetObject` by a session policy (AWS), both limited to the prefix |
| Google Drive sharing (`type = "rclone"`) | Who the shared drive is shared with may fetch its packs, with their own Google account. The private pack repository decides who sees the list; the drive decides who gets the bytes. Both must allow it |
| rclone | It runs with the user's Google credentials; install it from rclone.org or a trusted package manager |

The client verifies the manifest's SHA-256 and size, and the hashes of every file in the pack's own `carla-pack.json`, before installing.
Even if a file in storage is replaced, nothing is installed unless it matches the manifest.

## Countermeasures

| Threat | Countermeasure | Verified by |
| --- | --- | --- |
| Fetching without permission | The manifest and key are only available through the GitHub API, which returns 404 if not readable. Storage refuses requests without a key | `tests/flow.rs` `no_read_access_means_no_pack`, `python/tests/test_e2e.py` |
| GitHub token leaking (to storage) | The token is only sent to the GitHub API host. No `Authorization` is attached to the asset's redirect target (another host; reqwest strips it on cross-origin redirects). S3 only gets SigV4 | `tests/flow.rs` (requires no Authorization at the redirect target), the E2E's `leaked == []` |
| Tampered or mixed-up files in storage | Checked against the manifest's SHA-256 / size. On a mismatch the temporary file is deleted and an error raised (`checksum_mismatch`) | `tampered_downloads_are_discarded` |
| Manifest and contents disagreeing | Refused unless the extracted `carla-pack.json`'s name, version, base release and platform match the manifest | `tests/pack.rs` |
| Malicious tarball (`..`, absolute paths, links, unlisted files, duplicate members) | Only regular files and directories under a single top folder are accepted, and every file is checked against the manifest. Nothing is left in packs_dir on failure | `tests/pack.rs` |
| Handing out keys from a public repository | The key workflow fails unless the repository is private | `templates/pack-repo/.github/workflows/*` |
| Key permissions too broad | R2: `object-read-only` + prefix; AWS: `s3:GetObject` + prefix by a session policy. Never beyond the source's permissions (R2's parent token, AWS's role) | — |
| Long-lived key leaking | What is handed out is a temporary key expiring in 12 hours. The issuer's secret (Cloudflare API token) is an Actions secret; AWS uses OIDC with no secret | — |
| Expired key | A key with less than 2 minutes left is not used, and the error names the cause (the workflow has stopped) | `expired_or_missing_credentials_are_reported` |
| Endpoint substitution (SSRF-like redirection) | `endpoint` must be an https origin (http only for localhost), with no path, query or user info. Keys must be normalized relative paths | tests in `src/manifest.rs` |
| Public packs on Hugging Face | Anonymous fetch that sends no token at all (neither GitHub nor Hugging Face). Even if the repository's contents (`main`, etc.) are rewritten, nothing is installed unless it matches the manifest's SHA-256. As a public repository, anything placed there is assumed fetchable by anyone | `public_packs_come_from_hugging_face_without_any_token` |
| Packs on Google Drive read by the wrong drive | The drive and folder come from the manifest and are set on every rclone call (both, empty when absent), so a user's remote configured for another drive or folder cannot redirect the read. IDs are checked to be plain Drive IDs and the remote name a plain rclone name, so neither can smuggle rclone options | `pins_the_drive_and_folder_on_every_call`, `rclone_storage` |
| A file on Google Drive replaced | The size and the SHA-256 Google Drive reports are compared with the manifest before downloading, and the downloaded bytes again after; on publish, what Drive holds is checked before the entry is appended | `access_problems_have_their_own_codes`, `publish_uploads_through_rclone_and_checks_what_arrived` |
| Google credentials leaking | carla-pack never reads them: rclone holds them in its own config (`rclone config file`); carla-pack passes only the remote's name. A token pasted with `drive login --token-stdin` is handed to `rclone config create` as an argument (visible to the same machine's process list for that moment) | — |
| Local credentials | The saved token is in `config.json` (600). The storage key is kept only in memory and never appears in logs or errors (hidden even in `Debug`) | tests in `src/sigv4.rs` |

## Revocation and propagation delay

- **Removing a person**: once removed from the pack repository (or team), they can no longer fetch the manifest or new keys.
  However, **a key already fetched stays valid until it expires (up to 12 hours)**. That is longer than the old version's signed URLs (5 minutes).
- **Stopping access immediately**: disable the key source.
  - R2: delete / roll (rotate) the R2 API token that is the temporary key's parent. A temporary key cannot exceed its parent's permissions, so temporary keys made from it are expected to stop working too (**not verified in a real environment**; check this before relying on it).
    Set the new parent token's access key ID in `R2_PARENT_ACCESS_KEY_ID` and run the workflow manually.
  - AWS: "Revoke active sessions" on the IAM role invalidates sessions issued before that point.
- The lifetime can be shortened with the workflow's `TTL_SECONDS` / `role-duration-seconds` (shorten the cron interval as well).

## Google Drive storage

- **Revocation is immediate**: remove the user from the shared drive (or its group) and rclone's next request is refused.
  No key was handed out, so there is nothing that stays valid until it expires.
- **The read scope covers the whole Drive.** `carla-pack drive login` asks for `drive.readonly`, which can read every file the
  user can read, not only the packs. The token stays in the user's rclone config (a long-lived refresh token, like `gh`'s);
  protect it like their other credentials. A service account for CI should be a member of the pack drive only.
- **Audit logs are better than for S3**: the Workspace's Drive audit log records each download with the user's account.
- Google does not publish Drive's download limits; heavy parallel downloads of the same file can be refused for a while (`storage_error`).

## Residual risks and known limitations

- **Everyone who can read the repository can fetch the key, and can pass it on.** Whoever receives it can read every pack under that prefix until it expires.
  This is the same nature as the old version's signed URLs, but the scope (the whole prefix) and duration (12 hours) are wider.
  Put packs with different audiences in separate repositories and prefixes (for R2, a separate parent token also works).
- **Audit logs are coarser.** GitHub does not record who downloads a Release asset. The storage access log only records the temporary key's ID.
- **Token permissions**: `gh auth token` and OAuth App tokens (`repo` scope) carry broad permissions beyond the pack repositories.
  carla-pack only reads, but for a dedicated token use a GitHub App (Contents: read-only) or a fine-grained PAT (Contents: read on the target repositories).
- **The moment the key is replaced**: `gh release upload --clobber` deletes before uploading, so the asset is missing for a very short time. A fetch in that window gets a "please retry" error.
- **Public access on R2**: enabling public access on the bucket through r2.dev or a custom domain bypasses this mechanism and lets anyone read it. Disable it for private use.
- People with Write permission on the pack repository (and their workflows) can rewrite the manifest to point at another file. Users with a lock file pinned by SHA-256 are not affected.
