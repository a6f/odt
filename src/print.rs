//! Facilities for pretty-printing Devicetree Source.

use crate::parse::{Parsed, Rule};
use core::fmt::Write;
use std::collections::HashMap;

pub fn format(dts: Parsed) -> String {
    let mut pretty = PrettyPrinter::new();
    pretty.print(dts, Rule::EOI, Rule::EOI);
    pretty.out.buffer
}

#[derive(Default)]
struct IndentingWriter {
    buffer: String,
    indent: usize,
    /// The number of newlines we should emit before the next non-comment token.
    pending_newlines: usize,
    /// The current output line (numbered from 0).
    line: usize,
}

impl IndentingWriter {
    fn indent(&mut self, delta: isize) {
        let new = self.indent as isize + delta;
        assert!(new >= 0);
        self.indent = new as usize;
    }
    fn ensure_following_lines(&mut self, lines: usize) {
        self.pending_newlines = self.pending_newlines.max(lines);
    }
    fn ensure_lines(&mut self) {
        while self.pending_newlines > 0 {
            self.push('\n');
        }
    }
    fn ensure_line(&mut self) {
        if self.buffer.chars().next_back().unwrap_or('\n') != '\n' {
            self.push('\n');
        }
    }
    fn ensure_space(&mut self) {
        if !self
            .buffer
            .chars()
            .next_back()
            .unwrap_or('\n')
            .is_ascii_whitespace()
        {
            self.push(' ');
        }
    }
    fn push(&mut self, c: char) {
        self.write_char(c).unwrap();
    }
    fn column(&self) -> usize {
        if self.buffer.chars().next_back().unwrap_or('\n') == '\n' {
            self.indent
        } else {
            self.buffer.lines().next_back().unwrap_or("").len()
        }
    }
    fn line(&self) -> usize {
        self.line
    }
}

impl Write for IndentingWriter {
    fn write_str(&mut self, input: &str) -> core::fmt::Result {
        for (i, line) in input.split('\n').enumerate() {
            if i > 0 {
                self.buffer.write_char('\n')?;
                self.line += 1;
                self.pending_newlines = self.pending_newlines.saturating_sub(1);
            }
            if line.is_empty() {
                continue;
            }
            if self.buffer.chars().next_back().unwrap_or('\n') == '\n' {
                write!(self.buffer, "{:1$}", "", self.indent)?;
            }
            if i == 0 {
                self.buffer.write_str(line)?;
            } else {
                // If we get multiple lines to print at once, assume they're part of a C-style
                // comment, and reindent the interior lines.
                let line = line.trim_start();
                let prefix = if line.starts_with('*') { " " } else { "   " };
                write!(self.buffer, "{prefix}{line}")?;
            }
        }
        Ok(())
    }
}

struct PrettyPrinter {
    tabstop: isize,
    out: IndentingWriter,
    /// type of the last token printed
    last: Rule,
    /// input lines since last token
    seen_lines: usize,
    /// output position (line, column) of the last printed token of each type
    last_output_position: HashMap<Rule, (usize, usize)>,
}

impl PrettyPrinter {
    fn new() -> Self {
        Self {
            tabstop: 4,
            out: IndentingWriter::default(),
            last: Rule::EOI,
            seen_lines: 0,
            last_output_position: HashMap::new(),
        }
    }

    fn print(&mut self, p: Parsed, parent: Rule, next_sibling: Rule) {
        let last = self.last;
        let rule = p.as_rule();
        let text = p.as_str();

        if rule == Rule::EOI {
            // Avoid extra blank lines at EOF.
            self.out.ensure_line();
            return;
        }

        if rule == Rule::WHITESPACE || rule == Rule::IncludeWhitespace {
            self.seen_lines += text.chars().filter(|&c| c == '\n').count();
            return;
        }

        // Adjust indentation.  (Adjusting as we enter and exit interior rules would be simpler,
        // but because the grammar accepts comments anywhere, that will not correctly reindent
        // comments.)
        let indent = match (parent, rule) {
            (_, Rule::OpenNode) => 1,
            (_, Rule::CloseNode) => -1,
            (Rule::Prop, Rule::PropName) => 1,
            (Rule::Prop, Rule::Semicolon) => -1,
            _ => 0,
        };

        // Unindent this token and following tokens.
        if indent < 0 {
            self.out.indent(indent * self.tabstop);
        }

        let children: Vec<Parsed> = p.into_inner().collect();
        if !children.is_empty() {
            // This is an interior rule; visit each child in turn.
            let mut it = children.iter();
            while let Some(p) = it.next() {
                // Check if the next token (other than whitespace or comments) begins
                // a node of interest for line break insertion.
                let mut next = it
                    .clone()
                    .find(|p| !matches!(p.as_rule(), Rule::WHITESPACE | Rule::COMMENT))
                    .cloned();
                let mut next_rule = Rule::EOI;
                while let Some(p) = next {
                    next_rule = p.as_rule();
                    if matches!(next_rule, Rule::TopNode | Rule::ChildNode | Rule::Cells) {
                        break;
                    }
                    next = p.into_inner().next(); // left recurse
                }
                self.print(p.clone(), rule, next_rule);
            }
        } else {
            // This is a leaf rule with no children; print it.  (The grammar associates all text
            // with such rules, so this is enough to reproduce the input.)
            self.last = rule;

            // Only a comment on the same line is allowed to linger when a line break is pending.
            if self.seen_lines > 0 || !matches!(rule, Rule::BlockComment | Rule::LineComment) {
                // Preserve up to two newlines around comments.
                let keep_lines = match (last, rule) {
                    (Rule::LineComment, _) => 2,
                    (_, Rule::LineComment) => 2,
                    (Rule::BlockComment, _) => 2,
                    (_, Rule::BlockComment) => 2,
                    _ => 0,
                };
                let lines = self.seen_lines.min(keep_lines);
                self.out.ensure_following_lines(lines);
                self.out.ensure_lines();
                self.seen_lines = 0;
            }

            // Determine whether to insert horizontal space based on the last token printed.
            let prepend_space = match (last, rule) {
                (Rule::EOI, _) => false,
                (_, Rule::Semicolon) => false,
                (_, Rule::Comma) => false,
                (_, Rule::CloseCells) => false,
                // TODO: should be generic for UnaryOp
                (Rule::LogicalNot | Rule::BitwiseNot | Rule::Negate, _) => false,
                (Rule::OpenCells, _) => false,
                (_, Rule::CloseParen) => false,
                (Rule::OpenParen, _) => false,
                (_, Rule::CloseSquare) => false,
                (Rule::OpenSquare, _) => false,
                _ => true,
            };

            if prepend_space {
                self.out.ensure_space();
            }

            // Align "<" with previous output line.
            // TODO:  This would be nice for "//" as well, but require lookahead.
            // (Trailing comments after a semicolon also confuse matters.)
            let mut pos = (self.out.line(), self.out.column());
            let last_pos = self
                .last_output_position
                .get(&rule)
                .copied()
                .unwrap_or((0, 0));
            if rule == Rule::OpenCells && pos.0 == last_pos.0 + 1 && pos.1 < last_pos.1 {
                write!(self.out, "{:1$}", "", last_pos.1 - pos.1).unwrap();
                pos.1 = last_pos.1;
            }
            self.last_output_position.insert(rule, pos);
            // Unless we've passed a semicolon.
            if rule == Rule::Semicolon {
                self.last_output_position.clear();
            }

            write!(self.out, "{text}").unwrap();
        }

        // Indent following tokens but not this token.
        if indent > 0 {
            self.out.indent(indent * self.tabstop);
        }

        // Insert vertical whitespace between some pairings of adjacent rules.
        // NB:  RHS needs to be explicitly matched when next_sibling is computed.
        let lines = match (rule, next_sibling) {
            (Rule::Version, _) => 2,
            (Rule::OpenNode, _) => 1,
            (Rule::COMMENT, Rule::TopNode) => 0,
            (Rule::BlockComment, Rule::TopNode) => 1,
            (Rule::LineComment, Rule::TopNode) => 0,
            (_, Rule::TopNode) => 2,
            (Rule::COMMENT, Rule::ChildNode) => 0,
            (Rule::BlockComment, Rule::ChildNode) => 1,
            (Rule::LineComment, Rule::ChildNode) => 0,
            (_, Rule::ChildNode) => 2,
            (Rule::TopNode, _) => 2,
            (Rule::Include, _) => 1,
            (Rule::Semicolon, _) => 1,
            (Rule::Comma, Rule::Cells) => 1,
            _ => 0,
        };
        self.out.ensure_following_lines(lines);
    }
}

#[cfg(test)]
fn split_testdata(text: &str) -> Vec<String> {
    text.split_inclusive('\n')
        .collect::<Vec<_>>()
        .split(|&s| s.starts_with("--"))
        .map(|v| v.concat())
        .filter(|v| !v.is_empty())
        .collect()
}

#[test]
fn test_format() {
    for (index, (input, expected)) in core::iter::zip(
        split_testdata(include_str!("testdata/format.in")),
        split_testdata(include_str!("testdata/format.out")),
    )
    .enumerate()
    {
        let tree = crate::parse::parse_untyped(&input);
        let tree = tree.unwrap();
        let formatted = format(tree);
        // to renumber input:
        // print!("-- {index}\n{input}");
        // to regenerate expected output:
        // print!("-- {index}\n{formatted}");
        pretty_assertions::assert_eq!(
            formatted,
            expected,
            "formatted output for testcase {index} differs from expected"
        );
    }
}
