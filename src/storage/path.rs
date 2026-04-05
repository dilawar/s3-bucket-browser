use std::fmt;
use std::path::PathBuf;

use strum::{Display, EnumIs};

// ── StoragePath ───────────────────────────────────────────────────────────────

/// A location in any supported backend.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StoragePath {
    /// S3-compatible location: `s3://<bucket>/<prefix>`.
    /// `prefix` has no leading slash; directories end with `/`.
    S3 { bucket: String, prefix: String },
    /// Local filesystem directory (dev / testing).
    Local(PathBuf),
}

impl Default for StoragePath {
    fn default() -> Self {
        Self::S3 {
            bucket: String::new(),
            prefix: String::new(),
        }
    }
}

/// Join an S3 prefix with a child name, returning a normalised key segment.
///
/// Uses plain string operations rather than `object_store::path::Path::child()`
/// because `child()` treats the argument as a single segment and percent-encodes
/// any `/` characters, which causes double-encoding when the resulting string is
/// later passed back to `Path::from()` (e.g. multi-segment names produced by
/// folder uploads: `"photos/sub/file.jpg"`).
///
/// The caller is responsible for appending a trailing `/` when the result
/// should represent a directory.
fn s3_join(prefix: &str, name: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    let name = name.trim_start_matches('/');
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    }
}

impl StoragePath {
    /// Construct an S3 path from a bucket and a prefix.
    pub fn s3(bucket: impl Into<String>, prefix: impl Into<String>) -> Self {
        Self::S3 {
            bucket: bucket.into(),
            prefix: prefix.into(),
        }
    }

    /// Construct an S3 path pointing at the root of a bucket (empty prefix).
    pub fn s3_root(bucket: impl Into<String>) -> Self {
        Self::s3(bucket, String::new())
    }

    /// Parse an address-bar string. Strings starting with `s3://` are S3;
    /// everything else is treated as a local path.
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if let Some(rest) = s.strip_prefix("s3://") {
            match rest.split_once('/') {
                Some((bucket, prefix)) => Self::s3(bucket, prefix),
                None => Self::s3_root(rest),
            }
        } else {
            Self::Local(PathBuf::from(s))
        }
    }

    /// Returns `true` if this path represents a directory (S3 prefix or local dir).
    pub fn is_dir(&self) -> bool {
        match self {
            Self::S3 { prefix, .. } => prefix.is_empty() || prefix.ends_with('/'),
            Self::Local(p) => p.is_dir(),
        }
    }

    /// One level up, or `None` if already at the root.
    pub fn parent(&self) -> Option<Self> {
        match self {
            Self::Local(p) => p.parent().map(|p| Self::Local(p.to_path_buf())),
            Self::S3 { bucket, prefix } => {
                if prefix.is_empty() {
                    return None;
                }
                let trimmed = prefix.trim_end_matches('/');
                let new_prefix = match trimmed.rfind('/') {
                    Some(idx) => trimmed[..=idx].to_owned(),
                    None => String::new(),
                };
                Some(Self::s3(bucket, new_prefix))
            }
        }
    }

    /// Descend into a child directory named `name` (trailing `/` for S3).
    pub fn child(&self, name: &str) -> Self {
        match self {
            Self::Local(p) => Self::Local(p.join(name)),
            Self::S3 { bucket, prefix } => Self::s3(bucket, format!("{}/", s3_join(prefix, name))),
        }
    }

    /// Return the path to a child *file* named `name` (no trailing `/`).
    pub fn child_file(&self, name: &str) -> Self {
        match self {
            Self::Local(p) => Self::Local(p.join(name)),
            Self::S3 { bucket, prefix } => Self::s3(bucket, s3_join(prefix, name)),
        }
    }

    /// Breadcrumb segments: `(label, path_to_that_segment)` from root to here.
    pub fn breadcrumbs(&self) -> Vec<(String, Self)> {
        match self {
            Self::Local(p) => {
                let mut acc = PathBuf::new();
                p.components()
                    .map(|c| {
                        acc.push(c);
                        let label = match c {
                            std::path::Component::RootDir => "/".to_owned(),
                            other => other.as_os_str().to_string_lossy().into_owned(),
                        };
                        (label, Self::Local(acc.clone()))
                    })
                    .collect()
            }
            Self::S3 { bucket, prefix } => {
                let mut out = vec![(format!("s3://{bucket}"), Self::s3_root(bucket))];
                if !prefix.is_empty() {
                    let mut acc = String::new();
                    for segment in prefix.trim_end_matches('/').split('/') {
                        acc.push_str(segment);
                        acc.push('/');
                        out.push((segment.to_owned(), Self::s3(bucket, &acc)));
                    }
                }
                out
            }
        }
    }
}

impl fmt::Display for StoragePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local(p) => write!(f, "{}", p.display()),
            Self::S3 { bucket, prefix } if prefix.is_empty() => write!(f, "s3://{bucket}/"),
            Self::S3 { bucket, prefix } => write!(f, "s3://{bucket}/{prefix}"),
        }
    }
}

// ── Entry types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Display, EnumIs)]
pub enum EntryKind {
    #[strum(to_string = "directory")]
    Directory,
    #[strum(to_string = "file")]
    File,
}

impl EntryKind {
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Directory => "📁",
            Self::File => "📄",
        }
    }
}

#[derive(Debug, Clone)]
pub struct StorageEntry {
    /// Last path segment (no trailing slash).
    pub name: String,
    /// Full path to this entry.
    pub path: StoragePath,
    pub kind: EntryKind,
    /// Size in bytes; `None` for directories or when unavailable.
    pub size: Option<u64>,
    pub last_modified: Option<chrono::DateTime<chrono::Utc>>,
}

// ── Utilities ─────────────────────────────────────────────────────────────────

/// Sort entries in-place: directories first, then files; each group
/// case-insensitively alphabetical.
pub fn sort_entries(entries: &mut [StorageEntry]) {
    entries.sort_by(|a, b| match (&a.kind, &b.kind) {
        (EntryKind::Directory, EntryKind::File) => std::cmp::Ordering::Less,
        (EntryKind::File, EntryKind::Directory) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
}

/// Format byte count as a human-readable string, e.g. `"1.2 MB"`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut idx = 0usize;
    while value >= 1024.0 && idx + 1 < UNITS.len() {
        value /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[idx])
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── StoragePath::parse ───────────────────────────────────────────────────

    #[test]
    fn parse_s3_root() {
        assert_eq!(
            StoragePath::parse("s3://my-bucket"),
            StoragePath::s3_root("my-bucket")
        );
    }

    #[test]
    fn parse_s3_with_prefix() {
        assert_eq!(
            StoragePath::parse("s3://my-bucket/some/prefix/"),
            StoragePath::s3("my-bucket", "some/prefix/"),
        );
    }

    #[test]
    fn parse_local() {
        assert_eq!(
            StoragePath::parse("/home/user/data"),
            StoragePath::Local("/home/user/data".into()),
        );
    }

    // ── Display ──────────────────────────────────────────────────────────────

    #[test]
    fn display_s3_root() {
        assert_eq!(StoragePath::s3_root("bucket").to_string(), "s3://bucket/");
    }

    #[test]
    fn display_s3_with_prefix() {
        assert_eq!(
            StoragePath::s3("bucket", "a/b/").to_string(),
            "s3://bucket/a/b/"
        );
    }

    // ── parent ───────────────────────────────────────────────────────────────

    #[test]
    fn parent_of_root_is_none() {
        assert_eq!(StoragePath::s3_root("bucket").parent(), None);
    }

    #[test]
    fn parent_of_top_level_dir() {
        assert_eq!(
            StoragePath::s3("bucket", "top/").parent(),
            Some(StoragePath::s3_root("bucket")),
        );
    }

    #[test]
    fn parent_of_nested_dir() {
        assert_eq!(
            StoragePath::s3("bucket", "a/b/c/").parent(),
            Some(StoragePath::s3("bucket", "a/b/")),
        );
    }

    // ── child ────────────────────────────────────────────────────────────────

    #[test]
    fn child_appends_slash() {
        assert_eq!(
            StoragePath::s3_root("bucket").child("foo"),
            StoragePath::s3("bucket", "foo/"),
        );
    }

    #[test]
    fn child_nested() {
        assert_eq!(
            StoragePath::s3("bucket", "a/").child("b"),
            StoragePath::s3("bucket", "a/b/"),
        );
    }

    #[test]
    fn child_inserts_separator_when_prefix_missing_slash() {
        // Prefix without trailing `/` (e.g. typed manually in the address bar).
        assert_eq!(
            StoragePath::s3("bucket", "foo").child("bar"),
            StoragePath::s3("bucket", "foo/bar/"),
        );
    }

    #[test]
    fn child_file_normal() {
        assert_eq!(
            StoragePath::s3("bucket", "foo/").child_file("bar.png"),
            StoragePath::s3("bucket", "foo/bar.png"),
        );
    }

    #[test]
    fn child_file_inserts_separator_when_prefix_missing_slash() {
        // This was the reported bug: "foo" + "bar.png" → "foobar.png" instead of "foo/bar.png".
        assert_eq!(
            StoragePath::s3("bucket", "foo").child_file("bar.png"),
            StoragePath::s3("bucket", "foo/bar.png"),
        );
    }

    #[test]
    fn child_file_from_root() {
        assert_eq!(
            StoragePath::s3_root("bucket").child_file("file.txt"),
            StoragePath::s3("bucket", "file.txt"),
        );
    }

    // ── breadcrumbs ──────────────────────────────────────────────────────────

    #[test]
    fn breadcrumbs_root_only() {
        let crumbs = StoragePath::s3_root("bucket").breadcrumbs();
        assert_eq!(crumbs.len(), 1);
        assert_eq!(crumbs[0].0, "s3://bucket");
    }

    #[test]
    fn breadcrumbs_nested() {
        let crumbs = StoragePath::s3("bucket", "a/b/").breadcrumbs();
        assert_eq!(crumbs.len(), 3);
        assert_eq!(crumbs[0].0, "s3://bucket");
        assert_eq!(crumbs[1].0, "a");
        assert_eq!(crumbs[2].0, "b");
    }

    // ── human_size ───────────────────────────────────────────────────────────

    #[test]
    fn human_size_bytes() {
        assert_eq!(human_size(512), "512 B");
    }

    #[test]
    fn human_size_kilobytes() {
        assert_eq!(human_size(1024), "1.0 KB");
    }

    #[test]
    fn human_size_megabytes() {
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
    }

    #[test]
    fn human_size_gigabytes() {
        assert_eq!(human_size(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    // ── sort_entries ─────────────────────────────────────────────────────────

    #[test]
    fn sort_dirs_before_files() {
        let make = |name: &str, kind: EntryKind| StorageEntry {
            name: name.to_owned(),
            path: StoragePath::s3_root("b"),
            kind,
            size: None,
            last_modified: None,
        };
        let mut entries = vec![
            make("z_file", EntryKind::File),
            make("a_dir", EntryKind::Directory),
            make("b_file", EntryKind::File),
            make("m_dir", EntryKind::Directory),
        ];
        sort_entries(&mut entries);
        assert_eq!(entries[0].name, "a_dir");
        assert_eq!(entries[1].name, "m_dir");
        assert_eq!(entries[2].name, "b_file");
        assert_eq!(entries[3].name, "z_file");
    }
}
