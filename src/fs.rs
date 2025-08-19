//! Facilities for reading sources from the filesystem.

use crate::error::SourceError;
use core::fmt::Write;
use core::ops::Range;
use core::str::Utf8Error;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub trait Loader {
    /// Search for an include.  `relative_to` may specify one directory to check first.
    /// The result is the path found and its contents, or `None` if no matching file was found.
    fn find(&self, relative_to: Option<&Path>, included_path: &Path) -> Option<(&Path, &[u8])>;

    /// Read and cache a file.  If successful, the return value is a reference to
    /// the path and the data (both owned by `self`).  Otherwise `None` is returned.
    fn read(&self, path: PathBuf) -> Option<(&Path, &[u8])>;

    // TODO:  This should return a custom error type holding the path.  Use SourceError?
    fn find_utf8(
        &self,
        relative_to: Option<&Path>,
        included_path: &Path,
    ) -> Result<Option<(&Path, &str)>, Utf8Error> {
        Ok(match self.find(relative_to, included_path) {
            Some((path, bytes)) => Some((path, core::str::from_utf8(bytes)?)),
            None => None,
        })
    }

    // TODO:  This should return a custom error type holding the path.  Use SourceError?
    fn read_utf8(&self, path: PathBuf) -> Result<Option<(&Path, &str)>, Utf8Error> {
        Ok(match self.read(path) {
            Some((path, bytes)) => Some((path, core::str::from_utf8(bytes)?)),
            None => None,
        })
    }

    /// Files successfully read.
    fn positive_deps(&self) -> Vec<PathBuf>;

    /// Directories checked for files not found within them.
    /// (When any of these change, compilation results could change.)
    fn negative_deps(&self) -> Vec<PathBuf>;

    /// Produce a representation of the files accessed, in the format of a ninja depfile
    /// or the output of `cpp -MD` or `makedepend`.
    fn write_depfile(&self, goal: &str) -> String {
        let files = self.positive_deps();
        let dirs = self.negative_deps();
        let mut out = String::new();
        let escape = |s: &str| s.replace(' ', "\\ ");
        _ = write!(out, "{}:", escape(goal));
        for f in files {
            // Arguably we should exclude "<stdin>", but dtc does not.
            _ = write!(out, " {}", escape(&f.to_string_lossy()));
        }
        // TODO:  Make this optional?  Unclear if we want the rigor.
        for d in dirs {
            _ = write!(out, " {}", escape(&d.to_string_lossy()));
        }
        _ = writeln!(out);
        out
    }

    /// Given a memory range within a buffer cached by this loader, report the path of the
    /// corresponding file.
    fn path_of_buffer(&self, mem: Range<*const u8>) -> Option<PathBuf>;

    /// Annotate an error with the path of a source file owned by this buffer.
    fn annotate_error(&self, err: SourceError) -> SourceError {
        if err.path().is_some() {
            return err;
        }
        let path = self
            .path_of_buffer(err.buffer())
            .unwrap_or("<unknown>".into());
        err.with_path(&path)
    }
}

/// A do-nothing implementation for tests.
pub struct DummyLoader;

#[allow(unused_variables)]
impl Loader for DummyLoader {
    fn find(&self, relative_to: Option<&Path>, included_path: &Path) -> Option<(&Path, &[u8])> {
        None
    }
    fn read(&self, path: PathBuf) -> Option<(&Path, &[u8])> {
        None
    }
    fn positive_deps(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn negative_deps(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn path_of_buffer(&self, mem: Range<*const u8>) -> Option<PathBuf> {
        None
    }
}

/// A stateful helper for loading source files from a list of include directories to be searched.
pub struct LocalFileLoader {
    /// directories to search
    search_path: Vec<(PathBuf, PathBuf)>,
    /// cache of previously loaded files
    file_contents: Mutex<HashMap<PathBuf, Option<Vec<u8>>>>,
    /// existing parent directories of files observed not to exist
    parents_of_missing: Mutex<HashSet<PathBuf>>,
}

impl LocalFileLoader {
    /// Magic pathname which will be loaded by reading from stdin.
    pub const STDIN: &str = "<stdin>";

    pub fn new(search_path: Vec<PathBuf>) -> Self {
        let search_path = search_path
            .into_iter()
            .map(|p| (PathBuf::new(), p))
            .collect();
        Self::new_with_prefixed_path(search_path)
    }

    pub fn new_with_prefixed_path(search_path: Vec<(PathBuf, PathBuf)>) -> Self {
        Self {
            search_path,
            file_contents: Default::default(),
            parents_of_missing: Default::default(),
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
                parents_of_missing.insert(path.into());
                return;
            }
        }
    }
}

impl Loader for LocalFileLoader {
    fn find(&self, relative_to: Option<&Path>, included_path: &Path) -> Option<(&Path, &[u8])> {
        if let Some(dir) = relative_to {
            if let Some(result) = self.read(dir.join(included_path)) {
                return Some(result);
            }
        }
        for (prefix, dir) in &self.search_path {
            if let Ok(suffix) = included_path.strip_prefix(prefix) {
                if let Some(result) = self.read(dir.join(suffix)) {
                    return Some(result);
                }
            }
        }
        None
    }

    fn read<'a>(&'a self, path: PathBuf) -> Option<(&'a Path, &'a [u8])> {
        let mut file_contents = self.file_contents.lock().unwrap();
        let entry = file_contents.entry(path);
        // SAFETY:  We never erase items from the map.
        // The `entry.key()` PathBuf is either in the map, or will be moved into it.
        // PathBuf is "StableDeref": moving it does not move the heap buffer.
        let key: &'a Path = unsafe { core::mem::transmute(entry.key().as_path()) };
        match entry.or_insert_with(|| {
            let result = if key == Path::new(Self::STDIN) {
                use std::io::Read;
                let mut bytes = vec![];
                std::io::stdin().read_to_end(&mut bytes).map(|_| bytes).ok()
            } else {
                std::fs::read(key).ok()
            };
            if result.is_none() {
                self.track_parent_of_missing(key);
            }
            result
        }) {
            None => None,
            Some(value) => {
                // SAFETY:  We never erase items from the map.
                // Hashtable resizes may move the Vec, but Vec is "StableDeref":
                // moving it does not move the heap buffer.
                let value: &'a [u8] = unsafe { core::mem::transmute(value.as_slice()) };
                Some((key, value))
            }
        }
    }

    fn positive_deps(&self) -> Vec<PathBuf> {
        let mut deps: Vec<_> = self
            .file_contents
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(path, content)| content.is_some().then_some(path.clone()))
            .collect();
        deps.sort();
        deps
    }

    fn negative_deps(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<_> = self
            .parents_of_missing
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect();
        dirs.sort();
        dirs
    }

    fn path_of_buffer(&self, mem: Range<*const u8>) -> Option<PathBuf> {
        for (path, bytes) in self.file_contents.lock().unwrap().iter() {
            if let Some(bytes) = bytes {
                if bytes.as_ptr_range() == mem {
                    return Some(path.clone());
                }
            }
        }
        None
    }
}
