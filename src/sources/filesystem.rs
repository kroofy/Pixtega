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
use crate::types::{FetchedObject, IdentifiedObject, ObjectIdentity, UpstreamKey};

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

    async fn identify(&self, key: &UpstreamKey) -> Result<Option<IdentifiedObject>, SourceError> {
        let root = self.root.clone();
        let segments = key.segments.clone();
        let limits = self.limits;
        match tokio::time::timeout(
            self.limits.timeout,
            tokio::task::spawn_blocking(move || identify_blocking(&root, &segments, &limits)),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(unavailable("filesystem worker task failed")),
            Err(_) => Err(SourceError::Timeout),
        }
    }
}

fn unavailable(detail: &str) -> SourceError {
    SourceError::Unavailable {
        upstream_status: None,
        detail: detail.to_string(),
    }
}

struct ResolvedFile {
    path: PathBuf,
    metadata: std::fs::Metadata,
}

/// Walk `segments` beneath `root` component by component using
/// `symlink_metadata` so symlinks are detected, never followed.
fn resolve_file(
    root: &Path,
    segments: &[String],
    limits: &FetchLimits,
) -> Result<ResolvedFile, SourceError> {
    if segments.is_empty() {
        return Err(unavailable("empty upstream key"));
    }

    let mut path = root.to_path_buf();
    let mut metadata = None;
    for (index, segment) in segments.iter().enumerate() {
        path.push(segment);
        let step = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(SourceError::NotFound {
                    upstream_status: None,
                });
            }
            Err(_) => return Err(unavailable("filesystem metadata read failed")),
        };

        if step.file_type().is_symlink() {
            // Every symlink below the root is rejected, whether it points
            // inside or outside the root.
            return Err(unavailable("symlink in source path"));
        }

        let is_last = index + 1 == segments.len();
        if is_last {
            if !step.is_file() {
                return Err(unavailable("source path is not a regular file"));
            }
            if step.len() > limits.max_bytes {
                return Err(SourceError::TooLarge {
                    upstream_status: None,
                });
            }
            metadata = Some(step);
        } else if !step.is_dir() {
            // An intermediate component that is not a directory means the
            // full key cannot name an object: absence, not a fault.
            return Err(SourceError::NotFound {
                upstream_status: None,
            });
        }
    }

    Ok(ResolvedFile {
        path,
        metadata: metadata.expect("last segment always records metadata"),
    })
}

fn file_identity(segments: &[String], metadata: &std::fs::Metadata) -> Option<ObjectIdentity> {
    let mtime = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(ObjectIdentity::weak(format!(
        "{}:{}:{}",
        mtime,
        metadata.len(),
        segments.join("/")
    )))
}

fn fetch_blocking(
    root: &Path,
    segments: &[String],
    limits: &FetchLimits,
) -> Result<FetchedObject, SourceError> {
    let resolved = resolve_file(root, segments, limits)?;
    let identity = file_identity(segments, &resolved.metadata);
    let mut fetched = read_limited(&resolved.path, limits.max_bytes, resolved.metadata.len())?;
    fetched.identity = identity;
    Ok(fetched)
}

fn identify_blocking(
    root: &Path,
    segments: &[String],
    limits: &FetchLimits,
) -> Result<Option<IdentifiedObject>, SourceError> {
    let resolved = resolve_file(root, segments, limits)?;
    Ok(
        file_identity(segments, &resolved.metadata).map(|identity| IdentifiedObject {
            identity,
            upstream_status: None,
        }),
    )
}

/// Read the file in chunks, enforcing the byte limit again while reading
/// (the metadata check above may not reflect the bytes actually read).
fn read_limited(path: &Path, max_bytes: u64, size_hint: u64) -> Result<FetchedObject, SourceError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(SourceError::NotFound {
                upstream_status: None,
            });
        }
        Err(_) => return Err(unavailable("filesystem open failed")),
    };
    read_all_limited(file, max_bytes, size_hint)
}

/// The chunked read loop, split from the file open so the `Interrupted`
/// retry and the streaming byte limit can be exercised with an arbitrary
/// reader. `size_hint` preallocates from the file metadata (bounded by
/// `max_bytes` by the caller); the limit below stays authoritative.
fn read_all_limited(
    mut reader: impl Read,
    max_bytes: u64,
    size_hint: u64,
) -> Result<FetchedObject, SourceError> {
    let mut bytes: Vec<u8> = Vec::with_capacity(size_hint.min(max_bytes) as usize);
    let mut chunk = [0u8; READ_CHUNK_BYTES];
    loop {
        let read = match reader.read(&mut chunk) {
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
        identity: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader that yields scripted results, then EOF.
    struct ScriptedReader {
        script: Vec<Result<Vec<u8>, std::io::ErrorKind>>,
    }

    impl Read for ScriptedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.script.is_empty() {
                return Ok(0);
            }
            match self.script.remove(0) {
                Ok(data) => {
                    buf[..data.len()].copy_from_slice(&data);
                    Ok(data.len())
                }
                Err(kind) => Err(std::io::Error::from(kind)),
            }
        }
    }

    fn assert_unavailable(result: Result<FetchedObject, SourceError>, what: &str) {
        assert!(
            matches!(result, Err(SourceError::Unavailable { .. })),
            "{what}: expected Unavailable, got {result:?}"
        );
    }

    /// An `Interrupted` read is retried transparently; the data still
    /// arrives. Any other read error is unavailability, even when a retry
    /// would have produced data.
    #[test]
    fn interrupted_reads_are_retried_and_other_errors_are_unavailable() {
        let fetched = read_all_limited(
            ScriptedReader {
                script: vec![
                    Err(std::io::ErrorKind::Interrupted),
                    Ok(b"abc".to_vec()),
                    Err(std::io::ErrorKind::Interrupted),
                    Ok(b"def".to_vec()),
                ],
            },
            1024,
            0,
        )
        .expect("interrupted reads must be retried");
        assert_eq!(fetched.bytes, b"abcdef");

        assert_unavailable(
            read_all_limited(
                ScriptedReader {
                    script: vec![
                        Err(std::io::ErrorKind::PermissionDenied),
                        Ok(b"abc".to_vec()),
                    ],
                },
                1024,
                0,
            ),
            "non-Interrupted read error",
        );
    }

    /// The streaming limit is enforced on cumulative bytes: a first chunk
    /// alone may exceed it, and the boundary itself is accepted.
    #[test]
    fn streaming_byte_limit_is_cumulative_and_inclusive() {
        let over = read_all_limited(
            ScriptedReader {
                script: vec![Ok(b"12345".to_vec())],
            },
            4,
            0,
        );
        assert!(
            matches!(over, Err(SourceError::TooLarge { .. })),
            "5 bytes against a limit of 4 must be TooLarge, got {over:?}"
        );

        let exact = read_all_limited(
            ScriptedReader {
                script: vec![Ok(b"12".to_vec()), Ok(b"34".to_vec())],
            },
            4,
            0,
        )
        .expect("exactly max_bytes must be accepted");
        assert_eq!(exact.bytes, b"1234");
    }

    /// Open errors: absence maps to NotFound; any other open failure (here
    /// an invalid path with an embedded NUL) maps to unavailability, never
    /// to absence.
    #[test]
    fn open_errors_distinguish_absence_from_unavailability() {
        let dir = tempfile::tempdir().unwrap();
        let missing = read_limited(&dir.path().join("missing.jpg"), 1024, 0);
        assert!(
            matches!(
                missing,
                Err(SourceError::NotFound {
                    upstream_status: None
                })
            ),
            "missing file must be NotFound, got {missing:?}"
        );
        assert_unavailable(
            read_limited(&dir.path().join("nul\0byte"), 1024, 0),
            "invalid path open error",
        );
    }

    /// Metadata errors during the walk that are not NotFound (here an
    /// invalid path with an embedded NUL) are unavailability, not absence.
    #[test]
    fn metadata_errors_other_than_absence_are_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let limits = FetchLimits {
            max_bytes: 1024,
            timeout: std::time::Duration::from_secs(1),
            max_redirects: 0,
        };
        assert_unavailable(
            fetch_blocking(dir.path(), &["nul\0byte".to_string()], &limits),
            "invalid path metadata error",
        );
    }
}
