use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    /// GitHub answered 404: the repository (or file) does not exist, or the token cannot read it.
    #[error(
        "cannot read {0}: it does not exist or your GitHub account has no read access to it \
         (GitHub answers 404 for both); check `carla-pack whoami`"
    )]
    NoAccess(String),
    /// GitHub rejected the token.
    #[error("{0}; run `carla-pack login` or set GH_TOKEN")]
    Unauthenticated(String),
    #[error("GitHub error {status}: {message}")]
    GitHub { status: u16, message: String },
    /// The vended storage credentials are missing or expired.
    #[error("{0}")]
    Credentials(String),
    #[error("storage error {status}: {message}")]
    Storage { status: u16, message: String },
    /// rclone (the client of `type = "rclone"` storage) is not installed or cannot run.
    #[error("{0}")]
    RcloneUnavailable(String),
    /// The storage has no credentials for the user: no rclone remote, or Google rejected its token.
    #[error("{0}")]
    StorageUnauthenticated(String),
    /// The storage refused the user, or does not have the file: on Google Drive, a drive or
    /// folder that is not shared with the user's Google account looks the same as a missing one.
    #[error("{0}")]
    StorageNoAccess(String),
    /// Downloaded bytes do not match the SHA-256 or size in the manifest.
    #[error("{0}")]
    Integrity(String),
    /// Invalid `carla-packs.toml`.
    #[error("invalid manifest {0}")]
    Manifest(String),
    /// No pack release matches the reference / map name.
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Usage(String),
    #[error("interrupted")]
    Interrupted,
    /// The downloaded file is not a valid content pack (layout, manifest or checksums).
    #[error("{0}")]
    Pack(String),
    /// A valid pack built for another CARLA base release.
    #[error("{0}")]
    Incompatible(String),
    #[error("network error: {}", chain(.0))]
    Http(#[from] reqwest::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unexpected response: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Stable machine-readable code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoAccess(_) => "no_access",
            Self::Unauthenticated(_) => "unauthenticated",
            Self::GitHub { .. } => "github_error",
            Self::Credentials(_) => "credentials_unavailable",
            Self::Storage { .. } => "storage_error",
            Self::RcloneUnavailable(_) => "rclone_unavailable",
            Self::StorageUnauthenticated(_) => "storage_unauthenticated",
            Self::StorageNoAccess(_) => "storage_no_access",
            Self::Integrity(_) => "checksum_mismatch",
            Self::Manifest(_) => "invalid_manifest",
            Self::NotFound(_) => "not_found",
            Self::Interrupted => "interrupted",
            Self::Pack(_) => "invalid_pack",
            Self::Incompatible(_) => "incompatible_pack",
            Self::Usage(_) => "usage",
            Self::Http(_) => "network_error",
            Self::Io(_) => "io_error",
            Self::Json(_) => "unexpected_response",
        }
    }
}

/// "error sending request: connection refused" instead of only the outermost message.
fn chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        out.push_str(": ");
        out.push_str(&s.to_string());
        cur = s.source();
    }
    out
}
