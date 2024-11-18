fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filename = std::env::args().nth(1).unwrap();
    let source = std::fs::read_to_string(filename)?;
    let ast = dtsp::parse::parse(&source);
    print!("{}", dtsp::print::format(ast));
    Ok(())
}
