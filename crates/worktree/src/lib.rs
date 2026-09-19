//! The working directory as a snapshot (ticket 006): read a directory into a
//! `Files` map, and move a directory from one snapshot to another.
//!
//! Safety properties, all tested:
//! - the repository's own metadata directory (`.ghola`) is never read as
//!   content and never written to;
//! - paths coming out of a repository are untrusted: a component that is
//!   empty, `.`, `..`, or contains a path separator, a drive colon or a NUL
//!   is rejected before anything touches the disk, so a crafted tree cannot
//!   write outside the working directory;
//! - `apply` only ever touches paths that differ between the two snapshots,
//!   so untracked files the user has lying around are left alone;
//! - deleting the last file in a directory prunes the empty directory.
//!
//! Symbolic links are skipped (neither followed nor recorded). File modes
//! and empty directories are not tracked.

use std::fs;
use std::path::{Path, PathBuf};

use repo::Files;

/// The metadata directory inside a working directory.
pub const META_DIR: &str = ".ghola";

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("unsafe path {0:?}")]
    UnsafePath(String),
    #[error("path {0:?} is not valid UTF-8")]
    NonUtf8Path(PathBuf),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

fn io<T>(path: &Path, r: std::io::Result<T>) -> Result<T, WorktreeError> {
    r.map_err(|source| WorktreeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Turns a repository path into a location under `root`, rejecting anything
/// that could escape it.
pub fn safe_join(root: &Path, path: &[u8]) -> Result<PathBuf, WorktreeError> {
    let text = std::str::from_utf8(path)
        .map_err(|_| WorktreeError::UnsafePath(String::from_utf8_lossy(path).into_owned()))?;
    let mut out = root.to_path_buf();
    for comp in text.split('/') {
        let bad =
            comp.is_empty() || comp == "." || comp == ".." || comp.contains(['\\', ':', '\0']);
        if bad {
            return Err(WorktreeError::UnsafePath(text.to_string()));
        }
        out.push(comp);
    }
    if text.split('/').next() == Some(META_DIR) {
        return Err(WorktreeError::UnsafePath(text.to_string()));
    }
    Ok(out)
}

/// Every regular file under `root` (recursively), excluding the metadata
/// directory, keyed by `/`-separated relative path.
pub fn read_files(root: &Path) -> Result<Files, WorktreeError> {
    let mut out = Files::new();
    walk(root, &mut Vec::new(), true, &mut out)?;
    Ok(out)
}

fn walk(
    dir: &Path,
    prefix: &mut Vec<String>,
    top: bool,
    out: &mut Files,
) -> Result<(), WorktreeError> {
    let mut entries: Vec<_> = io(dir, fs::read_dir(dir))?
        .collect::<Result<_, _>>()
        .map_err(|source| WorktreeError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let name = e
            .file_name()
            .into_string()
            .map_err(|_| WorktreeError::NonUtf8Path(path.clone()))?;
        if top && name == META_DIR {
            continue;
        }
        let ft = io(&path, e.file_type())?;
        if ft.is_symlink() {
            continue;
        }
        prefix.push(name);
        if ft.is_dir() {
            walk(&path, prefix, false, out)?;
        } else if ft.is_file() {
            out.insert(prefix.join("/").into_bytes(), io(&path, fs::read(&path))?);
        }
        prefix.pop();
    }
    Ok(())
}

pub fn write_file(root: &Path, path: &[u8], content: &[u8]) -> Result<(), WorktreeError> {
    let full = safe_join(root, path)?;
    if let Some(parent) = full.parent() {
        io(parent, fs::create_dir_all(parent))?;
    }
    io(&full, fs::write(&full, content))
}

/// Deletes a file and then any directories it leaves empty (never `root`).
pub fn remove_file(root: &Path, path: &[u8]) -> Result<(), WorktreeError> {
    let full = safe_join(root, path)?;
    if full.is_file() {
        io(&full, fs::remove_file(&full))?;
    }
    let mut dir = full.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) {
            break;
        }
        if io(&d, fs::read_dir(&d))?.next().is_some() {
            break;
        }
        io(&d, fs::remove_dir(&d))?;
        dir = d.parent().map(Path::to_path_buf);
    }
    Ok(())
}

/// Moves the directory from snapshot `from` to snapshot `to`: deletes what
/// `to` lacks, then writes what is new or changed. Files in neither
/// snapshot (untracked) are untouched. Every path is validated before the
/// first write, so a bad path leaves the directory unchanged.
pub fn apply(root: &Path, from: &Files, to: &Files) -> Result<(), WorktreeError> {
    for path in from.keys().chain(to.keys()) {
        safe_join(root, path)?;
    }
    for path in from.keys().filter(|p| !to.contains_key(*p)) {
        remove_file(root, path)?;
    }
    for (path, content) in to {
        if from.get(path) != Some(content) {
            write_file(root, path, content)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(items: &[(&str, &str)]) -> Files {
        items
            .iter()
            .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
            .collect()
    }

    #[test]
    fn write_then_read_round_trips_and_skips_the_meta_dir() {
        let d = tempfile::tempdir().unwrap();
        let f = files(&[("a.txt", "1"), ("src/lib/x.rs", "x"), ("src/y", "")]);
        apply(d.path(), &Files::new(), &f).unwrap();
        fs::create_dir_all(d.path().join(META_DIR)).unwrap();
        fs::write(d.path().join(META_DIR).join("secret"), "meta").unwrap();
        assert_eq!(read_files(d.path()).unwrap(), f);
    }

    #[test]
    fn apply_touches_only_differing_paths_and_leaves_untracked_files() {
        let d = tempfile::tempdir().unwrap();
        let a = files(&[("keep", "1"), ("gone", "2"), ("dir/old", "3")]);
        let b = files(&[("keep", "1"), ("new", "4")]);
        apply(d.path(), &Files::new(), &a).unwrap();
        fs::write(d.path().join("untracked.txt"), "mine").unwrap();
        apply(d.path(), &a, &b).unwrap();
        let now = read_files(d.path()).unwrap();
        assert_eq!(
            now,
            files(&[("keep", "1"), ("new", "4"), ("untracked.txt", "mine")])
        );
        assert!(
            !d.path().join("dir").exists(),
            "the emptied directory is pruned"
        );
    }

    #[test]
    fn a_path_can_change_between_file_and_directory() {
        let d = tempfile::tempdir().unwrap();
        let a = files(&[("x", "file")]);
        let b = files(&[("x/inner", "dir")]);
        apply(d.path(), &Files::new(), &a).unwrap();
        apply(d.path(), &a, &b).unwrap();
        assert_eq!(read_files(d.path()).unwrap(), b);
        apply(d.path(), &b, &a).unwrap();
        assert_eq!(read_files(d.path()).unwrap(), a);
    }

    #[test]
    fn unsafe_paths_are_rejected_before_anything_is_written() {
        let d = tempfile::tempdir().unwrap();
        let ok = files(&[("fine", "1")]);
        for bad in [
            "../evil",
            "a/../../evil",
            "/abs",
            "a//b",
            "./a",
            "a\\b",
            "c:x",
            ".ghola/x",
            "",
        ] {
            let mut to = ok.clone();
            to.insert(bad.as_bytes().to_vec(), b"x".to_vec());
            assert!(
                matches!(
                    apply(d.path(), &Files::new(), &to),
                    Err(WorktreeError::UnsafePath(_))
                ),
                "{bad:?}"
            );
            assert!(
                !d.path().join("fine").exists(),
                "{bad:?}: nothing may be written first"
            );
        }
    }

    #[test]
    fn symlinks_are_skipped() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("real"), "1").unwrap();
        // Creating a symlink can need privileges on Windows; skip if it fails.
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.path().join("real"), d.path().join("link")).unwrap();
        assert!(read_files(d.path())
            .unwrap()
            .contains_key(b"real".as_slice()));
        #[cfg(unix)]
        assert!(!read_files(d.path())
            .unwrap()
            .contains_key(b"link".as_slice()));
    }
}
