#![allow(deprecated)]

use clap::Parser as _;
use odt::Arena;
use odt::error::Scribe;
use odt::fs::{Loader, LocalFileLoader};
use odt::line::LineTableCache;
use odt::merge::{NodeChange, PropChange, merge4};
use odt::parse::parse_with_includes;
use pest::Span;
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
    let (_tree, node_labels, node_changes, prop_changes) = merge4(&dts, &mut scribe);
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

    // show locations of all nodes
    for (path, history) in node_changes {
        println!("history of node {path}:");
        let mut exists = false;
        for change in history {
            let verb = match change {
                NodeChange::TopDelNode(_) | NodeChange::DelNode(_) => {
                    exists = false;
                    "deleted"
                }
                _ => {
                    if exists {
                        "updated"
                    } else {
                        exists = true;
                        "created"
                    }
                }
            };
            let (file, line, col) = source_location(&loader, &ltc, change.span());
            println!("  {verb} at {}:{line}:{col}", file.display());
        }
        println!();
    }

    // show locations of all properties
    for (path, history) in prop_changes {
        println!("history of property {path}:");
        let mut exists = false;
        for change in history {
            let verb = match change {
                PropChange::DelProp(_) | PropChange::TopDelNode(_) | PropChange::DelNode(_) => {
                    exists = false;
                    "deleted"
                }
                _ => {
                    if exists {
                        "updated"
                    } else {
                        exists = true;
                        "created"
                    }
                }
            };
            let (file, line, col) = source_location(&loader, &ltc, change.span());
            println!("  {verb} at {}:{line}:{col}", file.display());
        }
        println!();
    }

    Ok(())
}
