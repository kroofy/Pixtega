//! Filesystem Source adapter contract tests.
//!
//! Covers: nested paths, unicode and dots preserved, absence vs
//! unavailability, symlink rejection at every level (inside or outside the
//! root), and the byte limit.

use std::os::unix::fs::symlink;
use std::path::Path;
use std::time::Duration;

use pixtega::config::FilesystemSourceConfig;
use pixtega::errors::SourceError;
use pixtega::sources::filesystem::FilesystemSource;
use pixtega::sources::{FetchLimits, Source};
use pixtega::types::UpstreamKey;

fn limits(max_bytes: u64) -> FetchLimits {
    FetchLimits {
        max_bytes,
        timeout: Duration::from_secs(5),
        max_redirects: 3,
    }
}

fn source(root: &Path, max_bytes: u64) -> FilesystemSource {
    let config = FilesystemSourceConfig {
        mount: "fixtures".to_string(),
        key_prefix_segments: Vec::new(),
        root: root.to_path_buf(),
    };
    FilesystemSource::new(&config, limits(max_bytes)).expect("adapter construction")
}

fn key(segments: &[&str]) -> UpstreamKey {
    UpstreamKey::new(segments.iter().map(|s| s.to_string()).collect())
}

fn write(root: &Path, relative: &[&str], bytes: &[u8]) {
    let mut path = root.to_path_buf();
    for segment in relative {
        path.push(segment);
    }
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
}

#[tokio::test]
async fn nested_file_is_fetched_with_no_upstream_status() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &["a", "b", "photo.jpg"], b"jpeg-bytes");

    let fetched = source(dir.path(), 1024)
        .fetch(&key(&["a", "b", "photo.jpg"]))
        .await
        .expect("fetch succeeds");
    assert_eq!(fetched.bytes, b"jpeg-bytes");
    assert_eq!(fetched.upstream_status, None);
}

#[tokio::test]
async fn unicode_and_dots_in_names_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let cases: Vec<Vec<&str>> = vec![
        vec!["süb.dir.v2", "påth image.v1.jpg"],
        vec!["media", "..almost.dots..", "日本語 файл.png"],
        vec!["dotted.file.name.webp"],
    ];
    for (index, segments) in cases.iter().enumerate() {
        let body = format!("body-{index}");
        write(dir.path(), segments, body.as_bytes());
        let fetched = source(dir.path(), 1024)
            .fetch(&key(segments))
            .await
            .unwrap_or_else(|err| panic!("fetch of {segments:?} failed: {err:?}"));
        assert_eq!(fetched.bytes, body.as_bytes());
    }
}

#[tokio::test]
async fn missing_file_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &["a", "exists.jpg"], b"x");

    let err = source(dir.path(), 1024)
        .fetch(&key(&["a", "missing.jpg"]))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            SourceError::NotFound {
                upstream_status: None
            }
        ),
        "expected NotFound, got {err:?}"
    );
}

#[tokio::test]
async fn missing_intermediate_directory_is_not_found() {
    let dir = tempfile::tempdir().unwrap();

    let err = source(dir.path(), 1024)
        .fetch(&key(&["no", "such", "dir", "photo.jpg"]))
        .await
        .unwrap_err();
    assert!(matches!(err, SourceError::NotFound { .. }), "got {err:?}");
}

#[tokio::test]
async fn intermediate_regular_file_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &["blocker.txt"], b"i am a file, not a dir");

    let err = source(dir.path(), 1024)
        .fetch(&key(&["blocker.txt", "photo.jpg"]))
        .await
        .unwrap_err();
    assert!(matches!(err, SourceError::NotFound { .. }), "got {err:?}");
}

#[tokio::test]
async fn directory_at_final_component_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &["gallery", "photo.jpg"], b"x");

    let err = source(dir.path(), 1024)
        .fetch(&key(&["gallery"]))
        .await
        .unwrap_err();
    assert!(
        matches!(err, SourceError::Unavailable { .. }),
        "a directory must be unavailability, got {err:?}"
    );
}

#[tokio::test]
async fn symlink_to_file_inside_root_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &["real.jpg"], b"real-bytes");
    symlink(dir.path().join("real.jpg"), dir.path().join("link.jpg")).unwrap();

    let adapter = source(dir.path(), 1024);

    // The symlink is rejected even though its target is beneath the root...
    let err = adapter.fetch(&key(&["link.jpg"])).await.unwrap_err();
    assert!(
        matches!(err, SourceError::Unavailable { .. }),
        "symlink must be unavailability, got {err:?}"
    );

    // ...while the real file itself stays fetchable.
    let fetched = adapter.fetch(&key(&["real.jpg"])).await.unwrap();
    assert_eq!(fetched.bytes, b"real-bytes");
}

#[tokio::test]
async fn symlink_to_file_outside_root_is_rejected() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.jpg"), b"secret").unwrap();

    let dir = tempfile::tempdir().unwrap();
    symlink(
        outside.path().join("secret.jpg"),
        dir.path().join("leak.jpg"),
    )
    .unwrap();

    let err = source(dir.path(), 1024)
        .fetch(&key(&["leak.jpg"]))
        .await
        .unwrap_err();
    assert!(
        matches!(err, SourceError::Unavailable { .. }),
        "symlink escape must be unavailability, got {err:?}"
    );
}

#[tokio::test]
async fn symlinked_intermediate_directory_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &["real-dir", "photo.jpg"], b"x");
    symlink(dir.path().join("real-dir"), dir.path().join("link-dir")).unwrap();

    let adapter = source(dir.path(), 1024);

    let err = adapter
        .fetch(&key(&["link-dir", "photo.jpg"]))
        .await
        .unwrap_err();
    assert!(
        matches!(err, SourceError::Unavailable { .. }),
        "symlinked directory must be unavailability, got {err:?}"
    );

    // The same file through the real directory works.
    assert!(adapter
        .fetch(&key(&["real-dir", "photo.jpg"]))
        .await
        .is_ok());
}

#[tokio::test]
async fn oversized_file_is_too_large() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &["big.jpg"], &[0u8; 33]);

    let err = source(dir.path(), 32)
        .fetch(&key(&["big.jpg"]))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            SourceError::TooLarge {
                upstream_status: None
            }
        ),
        "expected TooLarge, got {err:?}"
    );
}

#[tokio::test]
async fn file_at_exact_limit_is_fetched() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![7u8; 32];
    write(dir.path(), &["fits.jpg"], &body);

    let fetched = source(dir.path(), 32)
        .fetch(&key(&["fits.jpg"]))
        .await
        .expect("exact-limit file must be fetched");
    assert_eq!(fetched.bytes, body);
}
