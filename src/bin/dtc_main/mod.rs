// This is in a mod.rs so cargo's "autobins" feature doesn't find it.

use clap::Parser as _;
use odt::parse::TypedRuleExt;
use odt::parse::rules::TopDef;
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

    /// Add a directory to the include search path.
    /// If the value has the form 'crate=dir', then directory 'dir' is only
    /// searched for the remainder of included paths which begin with 'crate/'.
    #[arg(short = 'i', long, value_name = "path")]
    include: Vec<String>,

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

pub fn dtc_main(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse_from(args);

    match args.in_format {
        Format::Dtb => dtb_input(args),
        Format::Dti | Format::Dts | Format::Dtv => dts_input(args),
    }
}

fn dtb_input(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    use odt::fs::{Loader, LocalFileLoader};
    let loader = LocalFileLoader::new(vec![]);
    let input = args.input_path.unwrap_or(LocalFileLoader::STDIN.into());
    let Some((_path, data)) = loader.read(input.clone()) else {
        panic!("can't read {input:?}");
    };
    let tree = odt::flat::deserialize(data)?;
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
    let search_path = args
        .include
        .into_iter()
        .map(|s| match s.split_once('=') {
            Some((p, d)) => (PathBuf::from(p), PathBuf::from(d)),
            None => (PathBuf::new(), PathBuf::from(s)),
        })
        .collect();
    let loader = LocalFileLoader::new_with_prefixed_path(search_path);
    let input = args.input_path.unwrap_or(LocalFileLoader::STDIN.into());
    let arena = odt::Arena::new();
    let mut scribe = odt::error::Scribe::new(args.treat_warnings_as_errors);
    let bytes = match args.out_format {
        Format::Dtb => {
            let tree = odt::compile(&loader, &arena, &[&input], &mut scribe);
            odt::flat::serialize(&tree)
        }
        Format::Dti => {
            // This shows the tree after /include/ directives are processed.
            let dts = odt::parse::parse_with_includes(&loader, &arena, &input, &mut scribe);
            let mut output = String::new();
            for top_def in dts.top_def {
                if let TopDef::Include(_) = top_def {
                    output.push_str("// ");
                }
                output.push_str(top_def.str());
                output.push('\n');
            }
            output.into_bytes()
        }
        Format::Dts => {
            // This shows the tree after /include/ directives and merge operations,
            // but before assigning phandles or evaluating expressions.
            let tree = odt::merge(&loader, &arena, &[&input], &mut scribe);
            let source = format!("/dts-v1/;{}/{tree};", tree.labels_as_display());
            // Reparse and pretty-print the output.
            let tree = odt::parse::parse_untyped(&source).unwrap();
            let output = odt::print::format(tree);
            output.into_bytes()
        }
        Format::Dtv => {
            // Lower all the way to binary node values, then convert back into source.
            // Types are lost in this process.
            let tree = odt::compile(&loader, &arena, &[&input], &mut scribe);
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
