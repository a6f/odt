use clap::Parser as _;
use std::path::PathBuf;

#[derive(clap::Parser)]
#[command(version)]
struct Args {
    /// Input files
    #[arg(value_name = "input_path")]
    input_path: Vec<PathBuf>,

    /// Edit files in place
    #[arg(short = 'i', long)]
    in_place: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.input_path.is_empty() {
        if args.in_place {
            return Err("cannot reformat stdin in-place".into());
        }
        let source = std::io::read_to_string(std::io::stdin())?;
        let tree = odt::parse::parse_untyped(&source)?;
        let output = odt::print::format(tree);
        print!("{output}");
        return Ok(());
    }
    let mut success = true;
    for filename in args.input_path {
        let source = match std::fs::read_to_string(filename.clone()) {
            Ok(source) => source,
            Err(err) => {
                eprintln!("reading {filename:?}: {err}");
                success = false;
                continue;
            }
        };
        let tree = match odt::parse::parse_untyped(&source) {
            Ok(tree) => tree,
            Err(err) => {
                eprintln!("parsing {filename:?}:\n{err}");
                success = false;
                continue;
            }
        };
        let output = odt::print::format(tree);
        if args.in_place {
            // TODO:  Write to a temporary file and rename over the original.
            // Reopening the file like this can lose data (if interrupted or the disk is full).
            std::fs::write(filename, output)?;
        } else {
            print!("{output}");
        }
    }
    if success {
        Ok(())
    } else {
        std::process::exit(1)
    }
}
