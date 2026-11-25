//! Types which represent fully-evaluated properties in a devicetree.
//!
//! Unlike the DTB / FDT binary representation of property values, type information is retained,
//! so integers, phandles, strings, etc can be distinguished reliably.

use crate::Number;
use crate::node::OptionDisplay;
use crate::path::NodePath;
use core::fmt;
use core::fmt::Write;
use std::io;
use std::path::{Path, PathBuf};

pub type Tuple = Vec<TupleItem>;

#[derive(Clone, Debug, PartialEq)]
pub enum TupleItem {
    Number(Number),
    NodeReference {
        path: NodePath,
        phandle: Option<u32>,
    },
    String(String),
    Bytes(Vec<u8>),
    BinaryInclude {
        filename: PathBuf,
        data: Vec<u8>,
    },
}

impl TupleItem {
    pub const fn from_u64(value: u64) -> Self {
        Self::Number(Number::from_u64(value))
    }

    pub const fn as_number(&self) -> Option<Number> {
        match self {
            &Self::Number(value) => Some(value),
            _ => None,
        }
    }

    pub const fn as_u64(&self) -> Option<u64> {
        match self {
            &Self::Number(value) => Some(value.to_u64()),
            _ => None,
        }
    }

    pub const fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub const fn as_node(&self) -> Option<&NodePath> {
        match self {
            Self::NodeReference { path, .. } => Some(path),
            _ => None,
        }
    }

    pub const fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(v) => Some(v.as_slice()),
            Self::BinaryInclude { data, .. } => Some(data.as_slice()),
            _ => None,
        }
    }

    pub fn as_file(&self) -> Option<&Path> {
        match self {
            Self::BinaryInclude { filename, .. } => Some(filename.as_path()),
            _ => None,
        }
    }

    pub fn append_dtb_repr<W: io::Write>(&self, out: &mut W) -> io::Result<()> {
        fn write_null_terminated<W: io::Write>(out: &mut W, bytes: &[u8]) -> io::Result<()> {
            out.write_all(bytes)?;
            out.write_all(b"\0")
        }
        match self {
            &Self::Number(value) => out.write_all(&(value.to_u64() as u32).to_be_bytes()),
            Self::NodeReference { phandle, path } => match phandle {
                Some(phandle) => out.write_all(&phandle.to_be_bytes()),
                None => write_null_terminated(out, path.display().as_bytes()),
            },
            Self::String(s) => write_null_terminated(out, s.as_bytes()),
            Self::Bytes(b) => out.write_all(b),
            Self::BinaryInclude { data, .. } => out.write_all(data),
        }
    }
}

impl fmt::Display for TupleItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(value) => {
                let int = value.to_u64();
                if *value == Number::from_u64(int) {
                    write!(f, "{int:#04x}")
                } else {
                    write!(f, "{value}")
                }
            }
            Self::String(s) => {
                write!(f, "\"")?;
                for b in s.as_bytes() {
                    write!(f, "{}", std::ascii::escape_default(*b))?;
                }
                write!(f, "\"")
            }
            Self::NodeReference { path, .. } => write!(f, "&{{{path}}}"),
            Self::Bytes(data) | Self::BinaryInclude { data, .. } => {
                write!(f, "[")?;
                for (i, b) in data.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{b:02x}")?;
                }
                write!(f, "]")
            }
        }
    }
}

impl OptionDisplay for Tuple {
    fn fmt_opt(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut f = String::new();
        let mut in_cells = false;
        for item in self {
            let next_cells = matches!(
                item,
                TupleItem::Number(_)
                    | TupleItem::NodeReference {
                        phandle: Some(_),
                        ..
                    }
            );
            if in_cells && !next_cells {
                _ = write!(f, ">, ");
            } else if !in_cells && next_cells {
                _ = write!(f, "<");
            } else if in_cells {
                _ = write!(f, " ");
            } else if !f.is_empty() {
                _ = write!(f, ", ");
            }
            _ = write!(f, "{item}");
            in_cells = next_cells;
        }
        if in_cells {
            _ = write!(f, ">");
        }
        Some(f)
    }
}

pub fn dtb_repr(tuple: &Tuple) -> Vec<u8> {
    let mut buf = vec![];
    for x in tuple {
        x.append_dtb_repr(&mut buf).unwrap();
    }
    buf
}

pub fn into_dtb_repr(tuple: Tuple) -> Vec<u8> {
    dtb_repr(&tuple)
}
