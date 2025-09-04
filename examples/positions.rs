use clap::Parser as _;
use odt::fs::{Loader, LocalFileLoader};
use odt::merge::merge;
use odt::node::Node;
use odt::parse::gen::TypedRule;
use odt::parse::parse_with_includes;
use odt::path::NodePath;
use odt::Arena;
use pest::Span;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(clap::Parser)]
struct Args {
    /// Input file
    #[arg(value_name = "input_path")]
    input_path: Option<PathBuf>,

    /// Add a directory to the include search path
    #[arg(short = 'i', long, value_name = "path")]
    include: Vec<PathBuf>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let loader = LocalFileLoader::new(args.include);
    let input = args.input_path.unwrap_or(LocalFileLoader::STDIN.into());
    let arena = Arena::new();
    let dts = parse_with_includes(&loader, &arena, &input).map_err(|e| loader.with_path(e))?;
    let (tree, node_labels, node_decls) = merge(&dts).map_err(|e| loader.with_path(e))?;
    let node_paths = paths(&tree);

    // we don't need `fn paths()` now that node_decls is returned from `merge()`
    assert_eq!(
        node_decls.keys().collect::<HashSet<_>>(),
        node_paths.iter().collect::<HashSet<_>>()
    );

    // show all source files used
    println!("source files:");
    for path in loader.positive_deps() {
        println!("  {}", path.display());
    }
    println!();

    // show all labeled nodes
    println!("node labels:");
    for (label, path) in node_labels {
        println!("  {label} -> {path}");
    }
    println!();

    let source_location = |span: &Span| {
        let buffer = span.get_input().as_bytes().as_ptr_range();
        let path = loader.path_of_buffer(buffer).unwrap();
        // XXX BEWARE XXX
        // This query is linear in the file size, so quadratic in a loop!
        // If multiple queries are needed, add a cache of line starts to loader.
        let (line, col) = span.start_pos().line_col();
        // The column value is in codepoints, not bytes.
        (path, line, col)
    };

    // show locations of all nodes and properties
    for path in node_paths {
        let node_decl = node_decls[&path];
        let (file, line, col) = source_location(node_decl.span());
        println!(
            "node {path} last defined at {}:{line}:{col}",
            file.display()
        );
        let node = tree.walk(path.segments()).unwrap();
        for (name, prop) in node.properties() {
            // Find the edit point to modify the value of this property.
            let span = match prop.prop_value {
                None => prop.semicolon.span(),
                Some(propvalue) => propvalue.labeled_value[0].value.span(),
            };
            let (file, line, col) = source_location(span);
            println!("  {name} defined at {}:{line}:{col}", file.display());
        }
        println!();
    }
    Ok(())
}

fn paths<P>(root: &Node<P>) -> Vec<NodePath> {
    let mut out = vec![];
    _paths(root, NodePath::root(), &mut out);
    out
}

fn _paths<P>(node: &Node<P>, path: NodePath, out: &mut Vec<NodePath>) {
    out.push(path.clone());
    for (name, child) in node.children() {
        _paths(child, path.join(name), out);
    }
}
