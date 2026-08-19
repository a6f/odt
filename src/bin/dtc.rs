use clap::Parser as _;
use odt::parse::TypedRuleExt;
use odt::parse::rules::{ChildDef, NodeBody, PropDef, TopDef, TypedRule};
use odt::path::NodePath;
use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

#[derive(clap::Parser)]
#[command(version, args_override_self = true)]
struct Args {
    /// Input file
    #[arg(value_name = "input_path")]
    input_path: Option<PathBuf>,

    /// Input format
    #[arg(short = 'I', long, value_name = "format", default_value = "dts")]
    in_format: Format,

    /// Output format
    #[arg(short = 'O', long, value_name = "format", default_value = "dtb")]
    out_format: Format,

    /// Output file (stdout if omitted)
    #[arg(short = 'o', long, value_name = "path")]
    out: Option<PathBuf>,

    /// Output dependency file
    #[arg(short = 'd', long, value_name = "path")]
    out_dependency: Option<PathBuf>,

    /// Add a directory to the include search path
    #[arg(short = 'i', long, value_name = "path")]
    include: Vec<PathBuf>,

    /// Sort output tree alphabetically.
    #[arg(short = 's', long)]
    sort: bool,

    #[arg(short = 'W', long)]
    treat_warnings_as_errors: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq)]
enum Format {
    /// devicetree blob
    Dtb,
    /// devicetree source with includes expanded
    Dti,
    /// devicetree source
    Dts,
    /// devicetree source with nodes merged and comments preserved
    Dtsc,
    /// fully-evaluated devicetree source
    Dtv,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    match args.in_format {
        Format::Dtb => dtb_input(args),
        Format::Dti | Format::Dts | Format::Dtsc | Format::Dtv => dts_input(args),
    }
}

fn dtb_input(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    use odt::fs::{Loader, LocalFileLoader};
    let loader = LocalFileLoader::new(args.include);
    let input = args.input_path.unwrap_or(LocalFileLoader::STDIN.into());
    let Some((_path, data)) = loader.read(input.clone()) else {
        panic!("can't read {input:?}");
    };
    let mut tree = odt::flat::deserialize(data)?;
    if args.sort {
        tree.sort();
    }
    let (goal, mut writer) = open_output(args.out)?;
    match args.out_format {
        Format::Dtb => {
            let dtb = odt::flat::serialize(&tree);
            writer.write_all(&dtb)?;
        }
        Format::Dti | Format::Dts | Format::Dtsc | Format::Dtv => {
            let source = format!("/dts-v1/;/{tree};");
            // Reparse and pretty-print the output.
            let tree = odt::parse::parse_untyped(&source).unwrap();
            let output = odt::print::format(tree);
            write!(writer, "{output}")?;
        }
    }
    if let Some(depfile) = args.out_dependency {
        let content = loader.write_depfile(&goal);
        std::fs::write(depfile, content)?;
    }
    Ok(())
}

fn dts_input(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    use odt::fs::{Loader, LocalFileLoader};
    let loader = LocalFileLoader::new(args.include);
    let input = args.input_path.unwrap_or(LocalFileLoader::STDIN.into());
    let arena = odt::Arena::new();
    let mut scribe = odt::error::Scribe::new(args.treat_warnings_as_errors);
    let bytes = match args.out_format {
        Format::Dtb => {
            let mut tree = odt::compile(&loader, &[&input], &mut scribe);
            if args.sort {
                tree.sort();
            }
            odt::flat::serialize(&tree)
        }
        Format::Dti => {
            // This shows the tree after /include/ directives are processed.
            let dts = odt::parse::parse_with_includes(&loader, &arena, &input, &mut scribe);
            dti_output(&dts).into_bytes()
        }
        Format::Dtsc => {
            // Like `dts` output, but comments from the input files are spliced back
            // into the merged tree before pretty-printing.
            let dts = odt::parse::parse_with_includes(&loader, &arena, &input, &mut scribe);
            dtsc_output(&dts, &mut scribe, args.sort).into_bytes()
        }
        Format::Dts => {
            // This shows the tree after /include/ directives and merge operations,
            // but before assigning phandles or evaluating expressions.
            let mut tree = odt::merge(&loader, &arena, &[&input], &mut scribe);
            if args.sort {
                tree.sort();
            }
            let source = format!("/dts-v1/;{}/{tree};", tree.labels_as_display());
            // Reparse and pretty-print the output.
            let tree = odt::parse::parse_untyped(&source).unwrap();
            let output = odt::print::format(tree);
            output.into_bytes()
        }
        Format::Dtv => {
            // Lower all the way to binary node values, then convert back into source.
            // Types are lost in this process.
            let mut tree = odt::compile(&loader, &[&input], &mut scribe);
            if args.sort {
                tree.sort();
            }
            let source = format!("/dts-v1/;/{tree};");
            // Reparse and pretty-print the output.
            let tree = odt::parse::parse_untyped(&source).unwrap();
            let output = odt::print::format(tree);
            output.into_bytes()
        }
    };
    let ok = scribe.report(&loader, &mut std::io::stderr());
    let (goal, mut writer) = open_output(args.out)?;
    writer.write_all(&bytes)?;
    if let Some(depfile) = args.out_dependency {
        let content = loader.write_depfile(&goal);
        std::fs::write(depfile, content)?;
    }
    if ok {
        Ok(())
    } else {
        Err("compilation failed".into())
    }
}

fn dti_output(dts: &odt::parse::rules::Dts) -> String {
    // Comments and whitespace between top-level definitions are not part of any
    // `TopDef`, so copy the source text separating each definition from the
    // previous one, too.
    // Also, since `/include/` expansion mixes definitions from several files into
    // one sequence, keep a separate "emitted" position for each source string.
    let mut emitted = std::collections::HashMap::<*const u8, usize>::new();
    let mut output = String::new();
    let mut last = None;
    for top_def in dts.top_def {
        let span = top_def.span();
        let src = span.get_input();
        let pos = emitted.get(&src.as_ptr()).copied().unwrap_or(0);
        if pos <= span.start() {
            // Copy the source text between the last definition and this one.
            output.push_str(&src[pos..span.start()]);
        }
        if let TopDef::Include(_) = top_def {
            output.push_str("// ");
            output.push_str(top_def.str());
            output.push('\n');
        } else {
            output.push_str(top_def.str());
        }
        emitted.insert(src.as_ptr(), span.end());
        last = Some(span);
    }
    // Copy the source text after the last definition, if any.
    if let Some(span) = last {
        output.push_str(&span.get_input()[span.end()..]);
    }
    output
}

/// Identifies a parse tree construct by the address of the source string it was parsed
/// from, and the byte offset of the construct within that string.
type SourceAddress = (usize, usize);

fn source_address(span: &pest::Span) -> SourceAddress {
    (span.get_input().as_ptr() as usize, span.start())
}

/// Produces a merged output like `-O dts` while preserving comments from the source string.
fn dtsc_output(dts: &odt::parse::rules::Dts, scribe: &mut odt::error::Scribe, sort: bool) -> String {
    use odt::merge::{MergeEvent, NodeCreate};

    // Collect the text separating constructs from the parse tree.
    // At the top level, text ahead of definitions that are not themselves emitted
    // (headers, includes, deletions, ...) accumulates onto the next node definition.
    let mut text = HashMap::new();
    let mut emitted = HashMap::<*const u8, usize>::new();
    let mut pending = String::new();
    let mut last_span = None;
    let mut has_header = false;
    for top_def in dts.top_def {
        let span = top_def.span();
        let src = span.get_input();
        let pos = emitted.get(&src.as_ptr()).copied().unwrap_or(0);
        if pos <= span.start() {
            pending.push_str(&src[pos..span.start()]);
        }
        // The trailing `Plugin?` in the Header rule makes pest consume any
        // comments and whitespace following the header into its span; don't
        // skip past them.
        let end = match top_def {
            TopDef::Header(header) => {
                has_header = true;
                header
                    .plugin
                    .map(|p| p.span().end())
                    .unwrap_or(header.version.span().end())
            }
            _ => span.end(),
        };
        emitted.insert(src.as_ptr(), end);
        last_span = Some(span);
        if let TopDef::TopNode(topnode) = top_def {
            text.insert(source_address(span), core::mem::take(&mut pending));
            dtsc_collect(topnode.node_body, &mut text);
        }
    }
    let trailing = last_span.map(|span| &span.get_input()[span.end()..]).unwrap_or("");

    // Merge, reattaching the collected text to what's been carried over, keyed by path.
    // Comments preceding a node or property definition are attached to that construct,
    // and are removed from the output if that construct is deleted later on. Last,
    // comments are concatenated when a node or property is defined more than once.
    let mut prop_lead = HashMap::<(NodePath, String), String>::new();
    let mut node_lead = HashMap::<NodePath, String>::new();
    let mut node_tail = HashMap::<NodePath, String>::new();
    let mut floating = String::new();
    let (mut tree, _node_labels) =
        odt::merge::merge_with_events(dts, scribe, &mut |event| match event {
            MergeEvent::AddNode { path, create } => {
                if let Some(t) = text.get(&source_address(create.span())) {
                    floating.push_str(t);
                }
                node_lead
                    .entry(path.clone())
                    .or_insert_with(|| core::mem::take(&mut floating));
                let body = match create {
                    NodeCreate::TopNode(topnode) => topnode.node_body,
                    NodeCreate::ChildNode(childnode) => childnode.node_body,
                };
                if let Some(t) = text.get(&source_address(body.close_node.span())) {
                    node_tail.entry(path.clone()).or_default().push_str(t);
                }
            }
            MergeEvent::AddProp { path, name, prop } => {
                let entry = prop_lead.entry((path.clone(), name.into())).or_default();
                entry.push_str(&core::mem::take(&mut floating));
                if let Some(t) = text.get(&source_address(prop.span())) {
                    entry.push_str(t);
                }
            }
            MergeEvent::DelProp { path, name, .. } => {
                prop_lead.remove(&(path.clone(), name.to_string()));
            }
            MergeEvent::DelNode { path, .. } => {
                node_lead.retain(|p, _| !p.starts_with(path));
                node_tail.retain(|p, _| !p.starts_with(path));
                prop_lead.retain(|(p, _), _| !p.starts_with(path));
            }
            _ => (),
        });
    if sort {
        tree.sort();
    }

    // Regenerate the merged source with the comments spliced back in.
    let root = NodePath::root();
    let mut source = String::new();
    if let Some(s) = node_lead.get(&root) {
        source.push_str(s);
    }
    if has_header {
        source.push_str("/dts-v1/;\n");
    }
    use core::fmt::Write;
    _ = writeln!(source, "{}/ {{", tree.labels_as_display());
    dtsc_emit(&mut source, &tree, &root, &prop_lead, &node_lead, &node_tail);
    if let Some(s) = node_tail.get(&root) {
        source.push_str(s);
    }
    source.push_str(&floating);
    source.push_str("};\n");
    source.push_str(trailing);

    // Reparse and pretty-print the output; the pretty-printer preserves comments.
    let tree = odt::parse::parse_untyped(&source).unwrap();
    odt::print::format(tree)
}

/// Records the source text preceding each property and child node, and the
/// source text between each body's last item and its closing brace (keyed
/// by the closing brace).
fn dtsc_collect<'i>(body: &'i NodeBody<'i>, text: &mut HashMap<SourceAddress, String>) {
    let src = body.span().get_input();
    let mut pos = body.open_node.span().end();

    // Text ahead of constructs that are not themselves emitted
    // (/delete-node/, /delete-property/) carries over to the next
    // emitted construct (i.e., the next property or child node, or the
    // closing brace if none follows).
    let mut carry = String::new();
    for prop_def in body.node_contents.prop_def {
        let span = prop_def.span();
        carry.push_str(&src[pos..span.start()]);
        if let PropDef::Prop(_) = prop_def {
            text.insert(source_address(span), core::mem::take(&mut carry));
        }
        pos = span.end();
    }
    for child_def in body.node_contents.child_def {
        let span = child_def.span();
        carry.push_str(&src[pos..span.start()]);
        if let ChildDef::ChildNode(childnode) = child_def {
            text.insert(source_address(span), core::mem::take(&mut carry));
            dtsc_collect(childnode.node_body, text);
        }
        pos = span.end();
    }
    let close = body.close_node.span();
    carry.push_str(&src[pos..close.start()]);
    text.insert(source_address(close), carry);
}

/// Regenerate the merged source with the comments spliced back in.
fn dtsc_emit(
    out: &mut String,
    node: &odt::SourceNode,
    path: &NodePath,
    prop_lead: &HashMap<(NodePath, String), String>,
    node_lead: &HashMap<NodePath, String>,
    node_tail: &HashMap<NodePath, String>,
) {
    use core::fmt::Write;
    for (name, prop) in node.properties() {
        if let Some(s) = prop_lead.get(&(path.clone(), name.clone())) {
            out.push_str(s);
        }
        out.push_str(prop.str());
        out.push('\n');
    }
    for (name, child) in node.children() {
        let child_path = path.join(name);
        if let Some(s) = node_lead.get(&child_path) {
            out.push_str(s);
        }
        _ = writeln!(out, "{}{name} {{", child.labels_as_display());
        dtsc_emit(out, child, &child_path, prop_lead, node_lead, node_tail);
        if let Some(s) = node_tail.get(&child_path) {
            out.push_str(s);
        }
        out.push_str("};\n");
    }
}

fn open_output(
    out: Option<PathBuf>,
) -> Result<(String, Box<dyn Write>), Box<dyn std::error::Error>> {
    Ok(match out {
        Some(path) => (
            (path.to_string_lossy().into_owned()),
            Box::new(BufWriter::new(std::fs::File::create(path)?)),
        ),
        None => ("-".into(), Box::new(BufWriter::new(std::io::stdout()))),
    })
}

#[test]
fn test_dtsi_merges_and_keeps_comments() {
    let source = "// head\n/dts-v1/;\n/ {\n\t// one\n\ta = <1>;\n};\n// two\n/ {\n\tb = <2>;\n\t// tail\n};\n// end\n";
    let arena = odt::Arena::new();
    let dts = odt::parse::parse_typed(source, &arena).unwrap();
    let mut scribe = odt::error::Scribe::new(true);

    let output = dtsc_output(dts, &mut scribe, false);
    let (warnings, errors) = scribe.into_inner();
    assert!(warnings.is_empty() && errors.is_empty());
    assert_eq!(output.matches("/ {").count(), 1);
    for comment in ["// head", "// one", "// two", "// tail", "// end"] {
        assert!(output.contains(comment), "missing {comment:?} in:\n{output}");
    }
    assert!(output.contains("a = <1>;") && output.contains("b = <2>;"));
}

#[test]
fn test_dti_keeps_comments() {
    let source = "// before\n/dts-v1/;\n// after\n/ {\n};\n// trailing\n";
    let arena = odt::Arena::new();
    let dts = odt::parse::parse_typed(source, &arena).unwrap();
    assert_eq!(dti_output(dts), source);

}