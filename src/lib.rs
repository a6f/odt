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

pub fn compile(
    loader: &impl fs::Loader,
    arena: &Arena,
    dts_paths: &[&std::path::Path],
    scribe: &mut error::Scribe,
) -> BinaryNode {
    let dts = parse::parse_concat_with_includes(loader, arena, dts_paths, scribe);
    let (tree, node_labels) = merge::merge(&dts, scribe);
    let tree = eval::resolve_incbin_paths(loader, arena, tree, scribe);
    eval::eval(tree, node_labels, loader, scribe)
}

pub fn compile_result(
    loader: &impl fs::Loader,
    arena: &Arena,
    dts_paths: &[&std::path::Path],
) -> Result<BinaryNode, error::SourceError> {
    let mut scribe = error::Scribe::new(false);
    let r = compile(loader, arena, dts_paths, &mut scribe);
    scribe.collect().map(|_| r)
}

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

pub fn merge_result<'a>(
    loader: &'a impl fs::Loader,
    arena: &'a Arena,
    dts_paths: &[&std::path::Path],
) -> Result<SourceNode<'a>, error::SourceError> {
    let mut scribe = error::Scribe::new(false);
    let r = merge(loader, arena, dts_paths, &mut scribe);
    scribe.collect().map(|_| r)
}
