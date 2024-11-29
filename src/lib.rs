pub mod error;
pub mod eval;
pub mod flat;
pub mod fs;
pub mod parse;
pub mod parse_untyped;
pub mod print;

#[derive(Debug, Default)]
pub struct Node {
    pub name: String,
    pub properties: Vec<Property>,
    pub children: Vec<Node>,
}

#[derive(Debug, Default)]
pub struct Property {
    pub name: String,
    pub value: Vec<u8>,
}
