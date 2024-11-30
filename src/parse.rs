//! Facilities for parsing DTS files and expanding "/include/" directives.

use crate::error::SourceError;
use crate::fs::Loader;
use pest_typed::TypedParser;
use pest_typed_derive::TypedParser;
use std::path::Path;

#[derive(TypedParser)]
#[grammar = "dts.pest"]
#[emit_rule_reference]
#[box_only_if_needed]
struct DtsParser;

// TODO:  The pest_typed_derive representation of choices is not great.
// You would expect a Rust enum, but the only enums are generic with variants named _0, _1, _2....
// See if pest3 does better.

pub use crate::parse::rules::Dts;
use crate::parse::rules::*;

pub fn parse(source: &str) -> Result<Dts, SourceError> {
    let dtsfile = DtsParser::try_parse::<DtsFile>(&source)?;
    Ok(dtsfile.Dts().clone())
}

pub fn parse_with_includes<'a>(loader: &'a Loader, path: &'_ Path) -> Result<Dts<'a>, SourceError> {
    let Some((_, src)) = loader.read_utf8(path.into()) else {
        // TODO:  presumably there is some kind of filesystem error we could propagate
        return Err(SourceError::new_unattributed(format!(
            "can't load file {path:?}"
        )));
    };
    let dts = parse(src).map_err(|e| e.with_path(path))?;
    // make an empty container to receive the merged tree
    let mut out = dts.clone();
    out.content.0.matched.content.clear();
    out.content.1.matched.content.clear();
    out.content.2.matched.content.clear();
    out.content.3.matched.content.clear();
    _visit_includes(loader, path, dts, &mut out)?;
    Ok(out)
}

fn _visit_includes<'a>(
    loader: &'a Loader,
    path: &'_ Path,
    mut dts: Dts<'a>,
    out: &'_ mut Dts<'a>,
) -> Result<(), SourceError> {
    let dir = Some(path.parent().unwrap());
    for include in dts.Include() {
        let ipath = include.IncludePath().span.as_str().trim_matches('"');
        let Some((ipath, src)) = loader.find_utf8(dir, &Path::new(ipath)) else {
            return Err(include.err("can't find include file on search path".into()));
        };
        let dts = parse(src)?;
        _visit_includes(loader, ipath, dts, out)?;
    }
    // accumulate fields into the output, other than Includes (tuple element 1)
    let it = dts.content.0.matched.content.drain(..);
    out.content.0.matched.content.extend(it);
    let it = dts.content.2.matched.content.drain(..);
    out.content.2.matched.content.extend(it);
    let it = dts.content.3.matched.content.drain(..);
    out.content.3.matched.content.extend(it);
    Ok(())
}

pub trait SpannedExt<'a, R: pest_typed::RuleType, T: pest_typed::Spanned<'a, R>> {
    fn str(&self) -> &'a str;
    fn err(&self, message: String) -> SourceError;
}

impl<'a, R: pest_typed::RuleType, T: pest_typed::Spanned<'a, R>> SpannedExt<'a, R, T> for T {
    fn str(&self) -> &'a str {
        self.span().as_str()
    }
    fn err(&self, message: String) -> SourceError {
        // convert pest_typed::Span to pest::Span
        let s = self.span();
        let span = pest::Span::new(s.get_input(), s.start(), s.end()).unwrap();
        SourceError::new(message, span)
    }
}
