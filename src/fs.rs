//! Facilities for reading sources from the filesystem.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A stateful helper for loading source files from a list of include directories to be searched.
/// TODO: look at clang::FileManager and clang::HeaderSearch
/// TODO: make this "FileLoader" and put the initial file in it too.  will simplify lifetimes.
pub struct IncludeLoader {
    /// The list of directories to search
    search_path: Vec<PathBuf>,
    /// cache of previously loaded files
    file_contents: Mutex<HashMap<PathBuf, Option<Vec<u8>>>>,
    /// existing parent directories of files observed not to exist
    parents_of_missing: Mutex<HashSet<PathBuf>>,
}

impl IncludeLoader {
    pub fn new(search_path: Vec<PathBuf>) -> Self {
        Self {
            search_path,
            file_contents: Default::default(),
            parents_of_missing: Default::default(),
        }
    }

    /// Search for an include.  `relative_to` may specify one directory to check first.
    /// The result is the path found and its contents, or `None` if no matching file was found.
    pub fn find(&self, relative_to: Option<&Path>, included_path: &Path) -> Option<(&Path, &[u8])> {
        let search_path = self.search_path.iter().map(PathBuf::as_ref);
        for dir in relative_to.into_iter().chain(search_path) {
            let path = dir.join(included_path);
            if let Some(result) = self.read(path) {
                return Some(result);
            }
        }
        None
    }

    /// Read and cache a file.  If successful, the return value is a reference to
    /// the path and the data (both owned by `self`).  Otherwise `None` is returned.
    pub fn read<'a>(&'a self, path: PathBuf) -> Option<(&'a Path, &'a [u8])> {
        let mut file_contents = self.file_contents.lock().unwrap();
        let entry = file_contents.entry(path);
        // SAFETY:  We never erase items from the map.
        // The `entry.key()` PathBuf is either in the map, or will be moved into it.
        // Hashtable resizes will only move the PathBuf, not its heap buffer.
        // (This depends on PathBuf/OsStr/Vec not implementing SSO or the like.
        //  We could wrap them in Rc<> to ensure they don't move.)
        let key: &'a Path = unsafe { core::mem::transmute(entry.key().as_path()) };
        match entry.or_insert_with(|| {
            let result = std::fs::read(key).ok();
            if result.is_none() {
                self.track_parent_of_missing(key);
            }
            result
        }) {
            None => None,
            Some(value) => {
                // SAFETY:  We never erase items from the map.
                // Hashtable resizes will only move the Vec, not its heap buffer.
                let value = unsafe { core::mem::transmute(value.as_slice()) };
                Some((key, value))
            }
        }
    }

    fn track_parent_of_missing(&self, mut path: &Path) {
        let mut parents_of_missing = self.parents_of_missing.lock().unwrap();
        while let Some(parent) = path.parent() {
            path = parent;
            if path.as_os_str().is_empty() {
                // final ancestor of a relative path is ""; translate to "."
                parents_of_missing.insert(PathBuf::from("."));
            }
            if parents_of_missing.contains(path) {
                return;
            }
            if path.exists() {
                parents_of_missing.insert(path.to_owned());
                return;
            }
        }
    }

    /// Produce a representation of the files accessed, in the format of a ninja depfile
    /// or the output of `cpp -MD` or `makedepend`.
    pub fn write_depfile(&self, goal: &str) -> String {
        let mut files: Vec<_> = self.file_contents.lock().unwrap().keys().cloned().collect();
        files.sort();
        let mut dirs: Vec<_> = self
            .parents_of_missing
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect();
        dirs.sort();
        let mut out = String::new();
        let escape = |s: &str| s.replace(' ', "\\ ");
        _ = write!(out, "{}:", escape(goal));
        for f in files {
            _ = write!(out, " {}", escape(&f.to_string_lossy()));
        }
        for d in dirs {
            _ = write!(out, " {}", escape(&d.to_string_lossy()));
        }
        _ = writeln!(out);
        out
    }
}
