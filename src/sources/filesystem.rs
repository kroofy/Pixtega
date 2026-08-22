//! Filesystem Source adapter.
//!
//! Resolves keys beneath a configured absolute root. Rejects every symlink
//! encountered below the root. A missing regular file is absence; a
//! directory or other non-regular file is Source unavailability. Enforces
//! the same byte limit as the HTTP(S) adapter.

use std::io::Read;
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::config::FilesystemSourceConfig;
use crate::errors::{ConfigError, SourceError};
use crate::sources::{FetchLimits, Source};
use crate::types::{FetchedObject, UpstreamKey};

/// Read files in bounded chunks so the byte limit is enforced while
/// reading, independent of what metadata reported.
const READ_CHUNK_BYTES: usize = 64 * 1024;

pub struct FilesystemSource {
    /// Absolute, canonicalized root directory (validated by configuration).
    root: PathBuf,
    limits: FetchLimits,
}

impl FilesystemSource {
    pub fn new(config: &FilesystemSourceConfig, limits: FetchLimits) -> Result<Self, ConfigError> {
        Ok(FilesystemSource {
            root: config.root.clone(),
            limits,
        })
    }
}

#[async_trait]
impl Source for FilesystemSource {
    async fn fetch(&self, key: &UpstreamKey) -> Result<FetchedObject, SourceError> {
        let root = self.root.clone();
        let segments = key.segments.clone();
        let limits = self.limits;
        tokio::task::spawn_blocking(move || fetch_blocking(&root, &segments, &limits))
            .await
            .map_err(|_| unavailable("filesystem worker task failed"))?
    }
}

fn unavailable(detail: &str) -> SourceError {
    SourceError::Unavailable {
        upstream_status: None,
        detail: detail.to_string(),
    }
}

/// Walk `segments` beneath `root` component by component using
/// `symlink_metadata` so symlinks are detected, never followed.
fn fetch_blocking(
    root: &Path,
    segments: &[String],
    limits: &FetchLimits,
) -> Result<FetchedObject, SourceError> {
    if segments.is_empty() {
        return Err(unavailable("empty upstream key"));
    }

    let mut path = root.to_path_buf();
    for (index, segment) in segments.iter().enumerate() {
        path.push(segment);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(SourceError::NotFound {
                    upstream_status: None,
                });
            }
            Err(_) => return Err(unavailable("filesystem metadata read failed")),
        };

        if metadata.file_type().is_symlink() {
            // Every symlink below the root is rejected, whether it points
            // inside or outside the root.
            return Err(unavailable("symlink in source path"));
        }

        let is_last = index + 1 == segments.len();
        if is_last {
            if !metadata.is_file() {
                return Err(unavailable("source path is not a regular file"));
            }
            if metadata.len() > limits.max_bytes {
                return Err(SourceError::TooLarge {
                    upstream_status: None,
                });
            }
        } else if !metadata.is_dir() {
            // An intermediate component that is not a directory means the
            // full key cannot name an object: absence, not a fault.
            return Err(SourceError::NotFound {
                upstream_status: None,
            });
        }
    }

    read_limited(&path, limits.max_bytes)
}

/// Read the file in chunks, enforcing the byte limit again while reading
/// (the metadata check above may not reflect the bytes actually read).
fn read_limited(path: &Path, max_bytes: u64) -> Result<FetchedObject, SourceError> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(SourceError::NotFound {
                upstream_status: None,
            });
        }
        Err(_) => return Err(unavailable("filesystem open failed")),
    };

    let mut bytes: Vec<u8> = Vec::new();
    let mut chunk = [0u8; READ_CHUNK_BYTES];
    loop {
        let read = match file.read(&mut chunk) {
            Ok(read) => read,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(unavailable("filesystem read failed")),
        };
        if read == 0 {
            break;
        }
        if bytes.len() as u64 + read as u64 > max_bytes {
            return Err(SourceError::TooLarge {
                upstream_status: None,
            });
        }
        bytes.extend_from_slice(&chunk[..read]);
    }

    Ok(FetchedObject {
        bytes,
        upstream_status: None,
    })
}
