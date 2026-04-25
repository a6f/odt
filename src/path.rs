use core::fmt::{Debug, Display, Formatter};
use core::ops::Bound;

/// A portable subset of PathBuf for working with &{...} DTS path references.
/// Slashes are replaced with nulls, making the inherited `impl Ord` sort parents before children.
/// The stored string is normalized and ends with a null, simplifying `starts_with()` queries.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodePath(String);

impl NodePath {
    pub fn display(&self) -> String {
        if self.is_root() {
            "/".into()
        } else {
            self.0.strip_suffix('\0').unwrap().replace('\0', "/")
        }
    }

    pub fn is_root(&self) -> bool {
        self.0.len() == 1
    }

    pub fn join(&self, suffix: &str) -> Self {
        let mut r = self.clone();
        r.push(suffix);
        r
    }

    pub fn leaf(&self) -> &str {
        self.0.rsplit_terminator('\0').next().unwrap()
    }

    pub fn parent(&self) -> Self {
        if self.is_root() {
            return self.clone();
        }
        Self(self.0[..self.0.len() - 1 - self.leaf().len()].into())
    }

    pub fn pop(&mut self) {
        if !self.is_root() {
            self.0.truncate(self.0.len() - 1 - self.leaf().len());
        }
    }

    pub fn push(&mut self, suffix: &str) {
        for segment in suffix.split('/').filter(|s| !s.is_empty()) {
            self.0.push_str(segment);
            self.0.push('\0');
        }
    }

    pub fn root() -> Self {
        Self("\0".into())
    }

    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split_terminator('\0').filter(|s| !s.is_empty())
    }

    pub fn starts_with(&self, prefix: &Self) -> bool {
        self.0.starts_with(&prefix.0)
    }

    pub fn descendants_end(&self) -> Bound<Self> {
        if self.is_root() {
            Bound::Unbounded
        } else {
            Bound::Excluded(Self(self.0[..self.0.len() - 1].to_string() + "\x01\x00"))
        }
    }
}

impl Debug for NodePath {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        Debug::fmt(&self.display(), f)
    }
}

impl Display for NodePath {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        Display::fmt(&self.display(), f)
    }
}
