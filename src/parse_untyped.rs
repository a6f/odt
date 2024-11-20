//! Facilities for parsing DTS files.
//!
//! This module exposes pest's untyped grammar, which is more convenient for
//! clients wishing to visit every character of the input.

use pest::iterators::Pair;
use pest::Parser;
use pest_derive::Parser;


#[derive(Parser)]
#[grammar = "dts.pest"]
struct DtsParser;

pub type Tree<'a> = Pair<'a, Rule>;

pub fn parse(source: &str) -> Tree {
    match DtsParser::parse(Rule::dtsfile, &source) {
        Ok(mut dts) => dts.next().unwrap(),
        Err(err) => panic!(
            "parsing failed:\n{}",
            err.renamed_rules(|rule| format!("{:?}", rule))
        ),
    }
}
