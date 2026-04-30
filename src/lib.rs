pub mod error;
pub mod eval;
pub mod flat;
pub mod fs;
pub mod label;
pub mod line;
pub mod merge;
pub mod node;
pub mod parse;
pub mod path;
pub mod print;

pub type Arena = bumpalo::Bump;
pub type SourceNode<'i> = node::Node<&'i parse::rules::Prop<'i>>;
pub type BinaryNode = node::Node<Vec<u8>>;

/// Compile one or more DTS files into a fully-evaluated tree of binary values.
/// `loader` resolves input paths and `/include/` or `/incbin/` directives.
/// `scribe` records warnings and errors encountered during compilation.
pub fn compile(
    loader: &impl fs::Loader,
    dts_paths: &[&std::path::Path],
    scribe: &mut error::Scribe,
) -> BinaryNode {
    let arena = Arena::new();
    let dts = parse::parse_concat_with_includes(loader, &arena, dts_paths, scribe);
    let (tree, node_labels, _, _) = merge::merge(&dts, scribe);
    let tree = eval::resolve_incbin_paths(loader, &arena, tree, scribe);
    eval::eval(tree, node_labels, loader, scribe)
}

/// Compile one or more DTS files into a fully-evaluated tree of binary values.
/// `loader` resolves input paths and `/include/` or `/incbin/` directives.
/// Only the first compilation error is reported.
pub fn compile_result(
    loader: &impl fs::Loader,
    dts_paths: &[&std::path::Path],
) -> Result<BinaryNode, error::SourceError> {
    let mut scribe = error::Scribe::new(false);
    let r = compile(loader, dts_paths, &mut scribe);
    scribe.collect().map(|_| r)
}

/// Compile a self-contained DTS into a fully-evaluated tree of binary values.
/// Only the first compilation error is reported.
pub fn compile_inmemory(source: &str) -> Result<BinaryNode, error::SourceError> {
    let arena = Arena::new();
    let mut scribe = error::Scribe::new(false);
    let dts = parse::parse_typed(source, &arena)?;
    let (tree, node_labels, _, _) = merge::merge(dts, &mut scribe);
    let r = eval::eval(tree, node_labels, &fs::DummyLoader, &mut scribe);
    scribe.collect().map(|_| r)
}

/// Merge one or more DTS files into an unevaluated tree of property definitions.
/// `loader` resolves input paths and `/include/` or `/incbin/` directives.
/// `arena` retains parse tree nodes.
/// `scribe` records warnings and errors encountered during compilation.
pub fn merge<'a>(
    loader: &'a impl fs::Loader,
    arena: &'a Arena,
    dts_paths: &[&std::path::Path],
    scribe: &mut error::Scribe,
) -> SourceNode<'a> {
    let dts = parse::parse_concat_with_includes(loader, arena, dts_paths, scribe);
    let tree = merge::merge(&dts, scribe).0;
    eval::resolve_incbin_paths(loader, arena, tree, scribe)
}

/// Like `merge()`, but only the first compilation error is reported.
pub fn merge_result<'a>(
    loader: &'a impl fs::Loader,
    arena: &'a Arena,
    dts_paths: &[&std::path::Path],
) -> Result<SourceNode<'a>, error::SourceError> {
    let mut scribe = error::Scribe::new(false);
    let r = merge(loader, arena, dts_paths, &mut scribe);
    scribe.collect().map(|_| r)
}
