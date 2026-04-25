use clap::Parser as _;
use odt::error::Scribe;
use odt::fs::{Loader, LocalFileLoader};
use odt::line::LineTableCache;
use odt::merge::{MergeEvent, merge_with_events};
use odt::parse::parse_with_includes;
use odt::parse::rules::{Prop, TypedRule};
use odt::path::NodePath;
use odt::{Arena, SourceNode};
use pest::Span;
use std::collections::BTreeMap;
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
    let mut scribe = Scribe::new(false);
    let dts = parse_with_includes(&loader, &arena, &input, &mut scribe);
    let mut last_map = BTreeMap::<NodePath, &Prop>::new();
    let mut last_tree = SourceNode::default();
    let (_tree, node_labels) = merge_with_events(&dts, &mut scribe, &mut |e| {
        // println!("processing {e:?}");
        match e {
            MergeEvent::AddNode { path, .. } => {
                last_tree.walk_insert(path.segments());
            }
            MergeEvent::AddProp { path, name, prop } => {
                last_map.insert(path.join(name), prop);
                last_tree
                    .walk_insert(path.segments())
                    .set_property(name, prop);
            }
            _ => {}
        }
    });
    _ = scribe.report(&loader, &mut std::io::stderr()); // print errors but continue

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

    let ltc = LineTableCache::default();

    fn source_location<'a>(
        loader: &'a LocalFileLoader,
        ltc: &LineTableCache<'a>,
        span: &Span<'a>,
    ) -> (PathBuf, usize, usize) {
        let buffer = span.get_input().as_bytes().as_ptr_range();
        let path = loader.path_of_buffer(buffer).unwrap();
        let (line, col) = ltc.start_line_col(span);
        // The column value is in codepoints, not bytes.
        (path, line, col)
    }

    // show locations of all properties via last_map
    println!("properties:");
    for (path, prop) in last_map {
        let (file, line, col) = source_location(&loader, &ltc, prop.span());
        println!("  {path} defined at {}:{line}:{col}", file.display());
    }
    println!();

    // show locations of all properties via last_tree
    println!("properties by node:");
    for (path, node) in last_tree.iter_preorder(NodePath::root()) {
        println!("  {path}:");
        for (name, prop) in node.properties() {
            let (file, line, col) = source_location(&loader, &ltc, prop.span());
            println!("    {name} defined at {}:{line}:{col}", file.display());
        }
        println!();
    }

    Ok(())
}
