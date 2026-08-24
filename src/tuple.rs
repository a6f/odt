use crate::node::OptionDisplay;
use crate::path::NodePath;
use core::fmt;
use core::fmt::Write;
use std::io;
use std::path::{Path, PathBuf};

pub type Tuple = Vec<TupleItem>;

#[derive(Clone, Debug, PartialEq)]
pub enum TupleItem {
    Number {
        bits: u8,
        value: u64,
    },
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
        Self::Number { bits: 64, value }
    }

    pub const fn as_u64(&self) -> Option<u64> {
        match self {
            &Self::Number { value, .. } => Some(value),
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
            &Self::Number { bits, value } => match bits {
                8 => out.write_all(&(value as u8).to_be_bytes()),
                16 => out.write_all(&(value as u16).to_be_bytes()),
                32 => out.write_all(&(value as u32).to_be_bytes()),
                64 => out.write_all(&value.to_be_bytes()),
                n => unreachable!("invalid /bits/ {n}"),
            },
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
            Self::Number { value, .. } => write!(f, "{value:#04x}"),
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
                for b in data {
                    write!(f, " {b:02x}")?;
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
        let mut bits = None;
        for item in self {
            let next_bits = match item {
                &TupleItem::Number { bits, .. } => Some(bits),
                TupleItem::NodeReference {
                    phandle: Some(_), ..
                } => Some(32),
                _ => None,
            };
            if bits != next_bits {
                if bits.is_some() {
                    _ = write!(f, ">, ");
                }
                match next_bits {
                    Some(32) => _ = write!(f, "<"),
                    Some(n) => _ = write!(f, "/bits/ {n} <"),
                    None => (),
                }
                bits = next_bits;
            } else if bits.is_some() {
                _ = write!(f, " ");
            } else if !f.is_empty() {
                _ = write!(f, ", ");
            }
            _ = write!(f, "{item}");
        }
        if bits.is_some() {
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
