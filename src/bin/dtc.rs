use clap::Parser as _;
use odt::parse::TypedRuleExt;
use odt::parse::rules::{TopDef, TypedRule};
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
    /// fully-evaluated devicetree source
    Dtv,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    match args.in_format {
        Format::Dtb => dtb_input(args),
        Format::Dti | Format::Dts | Format::Dtv => dts_input(args),
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
        Format::Dti | Format::Dts | Format::Dtv => {
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
fn test_dti_keeps_comments() {
    let source = "// before\n/dts-v1/;\n// after\n/ {\n};\n// trailing\n";
    let arena = odt::Arena::new();
    let dts = odt::parse::parse_typed(source, &arena).unwrap();
    assert_eq!(dti_output(dts), source);
}
