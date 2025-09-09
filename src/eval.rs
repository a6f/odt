//! Facilities for evaluating expressions and phandle references in a devicetree.

use crate::error::{Scribe, SourceError};
use crate::fs::Loader;
use crate::label::{LabelMap, LabelResolver};
use crate::parse::rules::*;
use crate::parse::{SpanExt, TypedRuleExt, parse_quoted_string};
use crate::path::NodePath;
use crate::{Arena, BinaryNode, SourceNode};
use core::str::CharIndices;
use hashlink::{LinkedHashMap, LinkedHashSet};
use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// Assigns phandles and evaluates expressions.
/// Call with the output of `crate::merge::merge()` or `resolve_incbin_paths()`.
pub fn eval(
    tree: SourceNode,
    node_labels: LabelMap,
    loader: &impl Loader,
    scribe: &mut Scribe,
) -> BinaryNode {
    let phandles = assign_phandles(&tree, &node_labels, scribe);
    let read_file = |path: &Path| match loader.read(path.to_owned()) {
        Some((_, data)) => Ok(data.to_vec()),
        None => Err(SourceError::new_unattributed(format!(
            "can't load file {path:?}"
        ))),
    };
    let mut tree = evaluate_expressions(tree, &node_labels, &phandles, read_file, scribe);
    // poke assigned phandle values into the final tree
    for (path, phandle) in phandles {
        tree.walk_mut(path.segments())
            .unwrap()
            .set_property("phandle", phandle.to_be_bytes().into());
    }
    tree
}

fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(not(unix))]
    return PathBuf::from(String::from_utf8_lossy(&bytes).into_owned());
    #[cfg(unix)]
    return PathBuf::from(&<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(bytes));
}

/// Locate all files referenced by incbin directives, cache their contents, and rewrite their
/// source nodes to cwd-relative or absolute paths (rather than relative to the including file).
/// Call with the output of `crate::merge::merge()`.
pub fn resolve_incbin_paths<'a>(
    loader: &'a impl Loader,
    arena: &'a Arena,
    tree: SourceNode<'a>,
    scribe: &mut Scribe,
) -> SourceNode<'a> {
    let mut map_v = |v: &'a Value| match v {
        Value::Incbin(incbin) => {
            let buffer = incbin.span().get_input().as_bytes().as_ptr_range();
            let source = loader.path_of_buffer(buffer);
            let path_bytes = match incbin.incbin_args.quoted_string.unescape() {
                Err(e) => {
                    scribe.err(e);
                    return v;
                }
                Ok(bytes) => bytes,
            };
            let path = path_from_bytes(&path_bytes);
            match loader.find(source.as_deref().and_then(Path::parent), &path) {
                Some((found, _)) => {
                    if found == path {
                        return v;
                    }
                    // TODO: check if this round-trips a UTF-8 path.
                    let new_path_bytes: Vec<u8> = found
                        .to_string_lossy()
                        .bytes()
                        .flat_map(std::ascii::escape_default)
                        .collect();
                    let new_path = String::from_utf8_lossy(&new_path_bytes);
                    let quoted_string = arena.alloc_str(&format!("\"{new_path}\""));
                    let quoted_string = parse_quoted_string(quoted_string, arena).unwrap();
                    let incbin_args = arena.alloc(IncbinArgs {
                        quoted_string,
                        ..*incbin.incbin_args
                    });
                    let incbin = arena.alloc(Incbin {
                        incbin_args,
                        ..**incbin
                    });
                    // XXX  This doesn't work for DTS output, because OptionDisplay calls
                    // PropValue::str(), which doesn't see the inner span edits.
                    &*arena.alloc(Value::Incbin(incbin))
                }
                None => {
                    scribe.err(incbin.err(format!("/incbin/ {path:?}: file not found")));
                    v
                }
            }
        }
        other => other,
    };
    let mut map_pv = |pv: &'a PropValue| -> &'a PropValue {
        let mut v = pv.labeled_value.to_vec();
        for lv in v.iter_mut() {
            *lv = arena.alloc(LabeledValue {
                value: map_v(lv.value),
                ..**lv
            });
        }
        arena.alloc(PropValue {
            labeled_value: arena.alloc_slice_copy(&v),
            ..*pv
        })
    };
    let mut map_p = |p: &'a Prop<'a>| match p.prop_value {
        Some(pv) if contains_incbin(pv) => arena.alloc(Prop {
            prop_value: Some(map_pv(pv)),
            ..*p
        }),
        _ => p,
    };
    tree.map_values(&mut map_p)
}

fn contains_incbin(pv: &PropValue) -> bool {
    pv.labeled_value
        .iter()
        .any(|lv| matches!(lv.value, Value::Incbin(_)))
}

type PhandleMap = LinkedHashMap<NodePath, u32>;

fn assign_phandles(root: &SourceNode, node_labels: &LabelMap, scribe: &mut Scribe) -> PhandleMap {
    let labels = &LabelResolver(node_labels, root);
    // Find the targets of all phandle references.
    let mut need_phandles = LinkedHashSet::<NodePath>::new();
    visit_phandle_references(labels, root, &NodePath::root(), &mut need_phandles, scribe);
    // Find existing phandle properties.
    let mut phandles = vec![];
    visit_node_phandles(root, &NodePath::root(), labels, &mut phandles, scribe);
    // TODO:  Report an error for duplicate phandles.
    // `visit_node_phandles()` could collect into a set keyed on phandle value.
    let taken: std::collections::HashSet<u32> = phandles.iter().map(|(_, v)| *v).collect();
    let mut next_phandle = 1u32;
    let mut phandles = PhandleMap::from_iter(phandles);
    for path in need_phandles {
        if phandles.contains_key(&path) {
            continue;
        }
        while taken.contains(&next_phandle) {
            next_phandle += 1;
        }
        phandles.insert(path, next_phandle);
        next_phandle += 1;
    }
    phandles
}

fn visit_phandle_references<P>(
    labels: &LabelResolver<P>,
    node: &SourceNode,
    path: &NodePath,
    need_phandles: &mut LinkedHashSet<NodePath>,
    scribe: &mut Scribe,
) {
    for (_, prop) in node.properties() {
        if let Some(propvalue) = prop.prop_value {
            for labeled_value in propvalue.labeled_value {
                if let Value::Cells(cells) = labeled_value.value {
                    for label_or_cell in cells.label_or_cell {
                        if let LabelOrCell::Cell(Cell::NodeReference(phandle)) = label_or_cell {
                            match labels.resolve(path, phandle) {
                                Ok(target) => {
                                    need_phandles.replace(target);
                                }
                                Err(e) => scribe.err(e),
                            }
                        }
                    }
                }
            }
        }
    }
    for (name, child) in node.children() {
        let child_path = path.join(name);
        visit_phandle_references(labels, child, &child_path, need_phandles, scribe);
    }
}

fn node_phandle<P>(
    node: &SourceNode,
    path: &NodePath,
    labels: &LabelResolver<P>,
) -> Result<Option<u32>, SourceError> {
    let Some(prop) = node.get_property("phandle") else {
        return Ok(None);
    };
    // Each expression must have length 4, and may contain zero phandle references, or one,
    // pointing to itself.
    let Some(propvalue) = prop.prop_value else {
        return Err(prop.err("phandle property is empty"));
    };
    // We want to know whether `lookup_phandle` is called, but `evaluate_propvalue()`
    // expects Fn, not FnMut.  Work around that with a Cell.
    let phandle_is_self_reference = std::cell::Cell::new(false);
    let lookup_phandle = |noderef: &NodeReference| {
        if &labels.resolve(path, noderef)? != path {
            Err(propvalue.err("phandle expression cannot reference another phandle"))
        } else {
            phandle_is_self_reference.set(true);
            Ok(0)
        }
    };
    let phandle = evaluate_propvalue(
        propvalue,
        |_| Err(propvalue.err("phandle expression cannot use a string node reference")),
        lookup_phandle,
        // dtc allows this, but there's no need for it.
        |_| Err(propvalue.err("phandle expression cannot use /incbin/")),
    )?;
    let n = phandle.len();
    if n != 4 {
        return Err(propvalue.err(format!("phandles must be u32, got {n} bytes")));
    };
    // `visit_phandle_references()` will find self-references, so we omit them.
    if phandle_is_self_reference.get() {
        return Ok(None);
    }
    let phandle = u32::from_be_bytes(phandle.try_into().unwrap());
    if phandle == 0 || phandle == 0xffff_ffff {
        return Err(propvalue.err(format!("phandle has reserved value {phandle:#x}")));
    }
    Ok(Some(phandle))
}

fn visit_node_phandles<P>(
    node: &SourceNode,
    path: &NodePath,
    labels: &LabelResolver<P>,
    out: &mut Vec<(NodePath, u32)>,
    scribe: &mut Scribe,
) {
    match node_phandle(node, path, labels) {
        Ok(Some(phandle)) => out.push((path.clone(), phandle)),
        Ok(None) => (),
        Err(e) => scribe.err(e),
    }
    for (name, child) in node.children() {
        let child_path = path.join(name);
        visit_node_phandles(child, &child_path, labels, out, scribe);
    }
}

fn evaluate_expressions(
    root: SourceNode,
    node_labels: &LabelMap,
    phandles: &PhandleMap,
    read_file: impl Fn(&Path) -> Result<Vec<u8>, SourceError>,
    scribe: &mut Scribe,
) -> BinaryNode {
    let old = root.clone();
    let labels = &LabelResolver(node_labels, &old);
    let read_file = |p: &Path| read_file(p);
    let mut eval = |loc: &NodePath, prop: &Prop| match prop.prop_value {
        None => vec![],
        Some(propvalue) => {
            let lookup_label = |noderef: &NodeReference| labels.resolve(loc, noderef);
            let lookup_phandle = |noderef: &NodeReference| {
                Ok(*phandles.get(&labels.resolve(loc, noderef)?).unwrap())
            };
            match evaluate_propvalue(propvalue, lookup_label, lookup_phandle, read_file) {
                Ok(v) => v,
                Err(e) => {
                    scribe.err(e);
                    // TODO:  Consider returning `node::Node<Result<Vec<u8>, SourceError>>`
                    // instead of this in-band signaling.
                    b"<ERROR>\0".to_vec()
                }
            }
        }
    };
    root.map_located_values(&NodePath::root(), &mut eval)
}

// TODO:  Accept Scribe here as well.  It's probably not useful to report more than one error, or
// perform partial evaluation, but we might want to return warnings, e.g. about integer overflow.
fn evaluate_propvalue(
    propvalue: &PropValue,
    lookup_label: impl Fn(&NodeReference) -> Result<NodePath, SourceError>,
    lookup_phandle: impl Fn(&NodeReference) -> Result<u32, SourceError>,
    read_file: impl Fn(&Path) -> Result<Vec<u8>, SourceError>,
) -> Result<Vec<u8>, SourceError> {
    let mut r = vec![];
    for labeled_value in propvalue.labeled_value {
        match labeled_value.value {
            Value::Cells(cells) => {
                let bits = match cells.bits {
                    None => 32,
                    Some(bits) => {
                        let n = bits.numeric_literal.eval()?;
                        match n {
                            8 | 16 | 32 | 64 => n,
                            _ => return Err(bits.err("bad bit width: must be 8, 16, 32, or 64")),
                        }
                    }
                };
                for label_or_cell in cells.label_or_cell {
                    let LabelOrCell::Cell(cell) = label_or_cell else {
                        continue;
                    };
                    let n = match cell {
                        Cell::NodeReference(noderef) => {
                            let phandle = lookup_phandle(noderef)?;
                            if bits != 32 {
                                return Err(noderef.err("phandle references need /bits/ == 32"));
                            }
                            phandle as u64
                        }
                        Cell::ParenExpr(expr) => expr.eval()?,
                        Cell::IntLiteral(lit) => lit.eval()?,
                    };
                    if bits < 64 {
                        // dtc warns if the lost bits are not all the same.
                        // We might also want to warn if they are ones but the value looks positive.
                        let sign_bits = (63 - bits) as u32;
                        let sign_extended = ((n as i64) << sign_bits >> sign_bits) as u64;
                        if n != sign_extended {
                            let err = cell.err(format!("value exceeds {bits} bits"));
                            let trunc = n & sign_extended;
                            let tchars = 2 + bits as usize / 4;
                            // TODO: Reporter interface for warnings.  Can't decorate span with file path
                            // here, and these are printed even if a more severe error occurs later.
                            eprintln!("Truncating value {n:#x} to {trunc:#0tchars$x}:\n{err}");
                        }
                    }
                    match bits {
                        8 => r.push(n as u8),
                        16 => r.extend((n as u16).to_be_bytes()),
                        32 => r.extend((n as u32).to_be_bytes()),
                        64 => r.extend(n.to_be_bytes()),
                        _ => unreachable!(),
                    }
                }
            }
            Value::QuotedString(quotedstring) => {
                let bytes = quotedstring.unescape()?;
                r.extend(&*bytes);
                r.push(0);
            }
            Value::NodeReference(noderef) => {
                let target = lookup_label(noderef)?;
                r.extend(target.display().as_bytes());
                r.push(0);
            }
            Value::ByteString(bytestring) => {
                for label_or_hex_byte in bytestring.label_or_hex_byte {
                    if let LabelOrHexByte::HexByte(hex_byte) = label_or_hex_byte {
                        let s = hex_byte.str();
                        let b = u8::from_str_radix(s, 16).unwrap(); // parser has already validated
                        r.push(b);
                    }
                }
            }
            Value::Incbin(incbin) => {
                if !incbin.incbin_args.numeric_literal.is_empty() {
                    return Err(incbin.err("/incbin/ (..., offset, length) unimplemented"));
                }
                let path_bytes = incbin.incbin_args.quoted_string.unescape()?;
                let path = path_from_bytes(&path_bytes);
                let bin = read_file(&path).unwrap();
                if r.is_empty() {
                    r = bin;
                } else {
                    r.extend(bin);
                }
            }
        }
    }
    Ok(r)
}

trait UnescapeExt<'a> {
    fn unescape(&self) -> Result<Cow<'a, [u8]>, SourceError>;
}

impl<'a> UnescapeExt<'a> for QuotedString<'a> {
    fn unescape(&self) -> Result<Cow<'a, [u8]>, SourceError> {
        self.trim_one().unescape()
    }
}

impl<'a> UnescapeExt<'a> for CharLiteral<'a> {
    fn unescape(&self) -> Result<Cow<'a, [u8]>, SourceError> {
        let r = self.trim_one().unescape()?;
        match r.len() {
            1 => Ok(r),
            n => Err(self.err(format!("char literal is {n} bytes, should be one byte"))),
        }
    }
}

impl<'a> UnescapeExt<'a> for pest::Span<'a> {
    fn unescape(&self) -> Result<Cow<'a, [u8]>, SourceError> {
        let s = self.as_str();
        if !s.contains('\\') {
            return Ok(Cow::Borrowed(s.as_bytes()));
        }
        fn push_char(r: &mut Vec<u8>, c: char) {
            match c.len_utf8() {
                1 => r.push(c as u8),
                _ => r.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
            }
        }
        fn take_hex<'a>(it: &mut CharIndices<'a>) -> Result<u8, &'a str> {
            let n = it
                .clone()
                .take(2)
                .take_while(|(_, c)| c.is_ascii_hexdigit())
                .count();
            let s = &it.as_str()[..n];
            it.take(n).last();
            u8::from_str_radix(s, 16).or(Err(s))
        }
        fn take_oct<'a>(it: &mut CharIndices<'a>) -> Result<u8, &'a str> {
            let n = it
                .clone()
                .take(3)
                .take_while(|(_, c)| c.is_digit(8))
                .count();
            let s = &it.as_str()[..n];
            it.take(n).last();
            // `dtc` will accept and discard a ninth bit, e.g. '\501' is 'A'.
            // We reject escapes above '\377'.
            u8::from_str_radix(s, 8).or(Err(s))
        }
        let mut r = Vec::<u8>::new();
        let mut it = s.char_indices();
        while let Some((_, c)) = it.next() {
            if c != '\\' {
                push_char(&mut r, c);
                continue;
            }
            let it0 = it.clone();
            let Some((_, c)) = it.next() else {
                // This should be unreachable due to the grammar.
                return Err(self.err_at(it.as_str(), "unterminated escape sequence"));
            };
            let b: u8 = match c {
                'a' => b'\x07',
                'b' => b'\x08',
                'f' => b'\x0c',
                'n' => b'\n',
                'r' => b'\r',
                't' => b'\t',
                'v' => b'\x0b',
                'x' => take_hex(&mut it).map_err(|s| self.err_at(s, "bad hex escape sequence"))?,
                '0'..'8' => {
                    it = it0; // back up one character
                    take_oct(&mut it).map_err(|s| self.err_at(s, "bad octal escape sequence"))?
                }
                c => {
                    push_char(&mut r, c);
                    continue;
                }
            };
            r.push(b);
        }
        Ok(Cow::Owned(r))
    }
}

/// Evaluate an expression or parse a literal.
trait EvalExt {
    fn eval(&self) -> Result<u64, SourceError>;
}

impl EvalExt for IntLiteral<'_> {
    fn eval(&self) -> Result<u64, SourceError> {
        match self {
            IntLiteral::CharLiteral(c) => {
                let bytes = c.unescape()?;
                // This is a C 'char'; it has one byte.
                Ok(bytes[0].into())
            }
            IntLiteral::NumericLiteral(n) => n.eval(),
        }
    }
}

impl EvalExt for NumericLiteral<'_> {
    fn eval(&self) -> Result<u64, SourceError> {
        let s = self.str().trim_end_matches(['U', 'L']); // dtc is case-sensitive here
        parse_int(s).ok_or_else(|| self.err("bad numeric literal"))
    }
}

fn parse_int(s: &str) -> Option<u64> {
    if s == "0" {
        return Some(0);
    };
    let (digits, radix) = if let Some(hex) = s.strip_prefix("0x").or(s.strip_prefix("0X")) {
        (hex, 16)
    } else if let Some(oct) = s.strip_prefix('0') {
        (oct, 8)
    } else {
        (s, 10)
    };
    u64::from_str_radix(digits, radix).ok()
}

impl EvalExt for ParenExpr<'_> {
    fn eval(&self) -> Result<u64, SourceError> {
        self.expr.eval()
    }
}

impl EvalExt for Expr<'_> {
    fn eval(&self) -> Result<u64, SourceError> {
        self.ternary_prec.eval()
    }
}

impl EvalExt for UnaryExpr<'_> {
    fn eval(&self) -> Result<u64, SourceError> {
        let arg = self.unary_prec.eval()?;
        match self.unary_op {
            UnaryOp::LogicalNot(_) => Ok((arg == 0).into()),
            UnaryOp::BitwiseNot(_) => Ok(!arg),
            // Devicetree has only unsigned arithmetic, so negation is allowed to overflow.
            UnaryOp::Negate(_) => Ok(arg.wrapping_neg()),
        }
    }
}

impl EvalExt for TernaryPrec<'_> {
    fn eval(&self) -> Result<u64, SourceError> {
        let left = self.logical_or_prec.eval()?;
        let [mid, right] = self.expr else {
            return Ok(left);
        };
        // Note that subexpression evaluation is lazy, unlike dtc.
        if left != 0 { mid.eval() } else { right.eval() }
    }
}

macro_rules! impl_binary_eval {
    ($rule:ident, $op:ident, $arg:ident) => {
        impl EvalExt for $rule<'_> {
            fn eval(&self) -> Result<u64, SourceError> {
                let mut left = self.$arg[0].eval();
                for (op, right) in core::iter::zip(self.$op, &self.$arg[1..]) {
                    let right = right.eval()?;
                    // It would be nice to match on the type of `op` rather than its text, but to
                    // get the compile-time safety of an exhaustive match, we'd need one match
                    // statement per precedence rule.
                    left = eval_binary_op(left?, op.str(), right).map_err(|msg| self.err(msg));
                }
                left
            }
        }
    };
}

// TODO:  Should these short-circuit?
impl_binary_eval!(LogicalOrPrec, logical_or, logical_and_prec);
impl_binary_eval!(LogicalAndPrec, logical_and, bitwise_or_prec);

impl_binary_eval!(BitwiseOrPrec, bitwise_or, bitwise_xor_prec);
impl_binary_eval!(BitwiseXorPrec, bitwise_xor, bitwise_and_prec);
impl_binary_eval!(BitwiseAndPrec, bitwise_and, equal_prec);
impl_binary_eval!(EqualPrec, equal_prec_op, compare_prec);
impl_binary_eval!(ComparePrec, compare_prec_op, shift_prec);
impl_binary_eval!(ShiftPrec, shift_prec_op, add_prec);
impl_binary_eval!(AddPrec, add_prec_op, mul_prec);
impl_binary_eval!(MulPrec, mul_prec_op, unary_prec);

impl EvalExt for UnaryPrec<'_> {
    fn eval(&self) -> Result<u64, SourceError> {
        match self {
            UnaryPrec::UnaryExpr(x) => x.eval(),
            UnaryPrec::ParenExpr(x) => x.eval(),
            UnaryPrec::IntLiteral(x) => x.eval(),
        }
    }
}

fn eval_binary_op(left: u64, op: &str, right: u64) -> Result<u64, &'static str> {
    fn check(checked_or_wrapping: Result<u64, u64>) -> Option<u64> {
        match checked_or_wrapping {
            Ok(checked) => Some(checked),
            Err(wrapping) => cfg!(feature = "wrapping-arithmetic").then_some(wrapping),
        }
    }
    fn add(a: u64, b: u64) -> Option<u64> {
        check(a.checked_add(b).ok_or(a.wrapping_add(b)))
    }
    fn sub(a: u64, b: u64) -> Option<u64> {
        check(a.checked_sub(b).ok_or(a.wrapping_sub(b)))
    }
    fn mul(a: u64, b: u64) -> Option<u64> {
        check(a.checked_mul(b).ok_or(a.wrapping_mul(b)))
    }
    fn shl(a: u64, b: u64) -> u64 {
        if b < 64 { a << b } else { 0 }
    }
    fn shr(a: u64, b: u64) -> u64 {
        if b < 64 { a >> b } else { 0 }
    }
    match op {
        "+" => add(left, right).ok_or("arithmetic overflow"),
        "-" => sub(left, right).ok_or("arithmetic overflow"),
        "*" => mul(left, right).ok_or("arithmetic overflow"),
        "<<" => Ok(shl(left, right)),
        ">>" => Ok(shr(left, right)),
        "/" => left.checked_div(right).ok_or("division by zero"),
        "%" => left.checked_rem(right).ok_or("division by zero"),
        "&" => Ok(left & right),
        "|" => Ok(left | right),
        "^" => Ok(left ^ right),
        "&&" => Ok((left != 0 && right != 0) as u64),
        "||" => Ok((left != 0 || right != 0) as u64),
        "<=" => Ok((left <= right) as u64),
        ">=" => Ok((left >= right) as u64),
        "<" => Ok((left < right) as u64),
        ">" => Ok((left > right) as u64),
        "==" => Ok((left == right) as u64),
        "!=" => Ok((left != right) as u64),
        _ => Err("unknown binary operator"),
    }
}

#[test]
fn test_eval() {
    for source in [
        include_str!("testdata/charlit.dts"),
        include_str!("testdata/expr.dts"),
        include_str!("testdata/phandle.dts"),
        #[cfg(feature = "wrapping-arithmetic")]
        include_str!("testdata/random_expressions.dts"),
        include_str!("testdata/references.dts"),
    ] {
        let loader = crate::fs::DummyLoader;
        let arena = crate::Arena::new();
        let dts = crate::parse::parse_typed(source, &arena).unwrap();
        let mut scribe = Scribe::new(true);
        let (tree, node_labels, _, _) = crate::merge::merge(&dts, &mut scribe);
        let tree = eval(tree, node_labels, &loader, &mut scribe);
        assert!(scribe.report(&loader, &mut std::io::stderr()));
        let check = tree.get_child("check").unwrap_or(&tree);
        for (name, value) in check.properties() {
            if let Some((s1, s2)) = split_strs(value) {
                assert_eq!(
                    s1, s2,
                    "property {name} did not evaluate to two equal strings"
                );
            } else if value.len() == 8 {
                // Special-case two u32 cells.
                let left = u32::from_be_bytes(value[0..4].try_into().unwrap());
                let right = u32::from_be_bytes(value[4..8].try_into().unwrap());
                assert_eq!(
                    left, right,
                    "property {name} did not evaluate to two equal cells"
                );
            } else {
                let mid = value.len() / 2;
                assert_eq!(
                    &value[..mid],
                    &value[mid..],
                    "property {name} did not evaluate to two equal byte strings"
                );
            }
        }
    }
}

#[cfg(test)]
fn split_strs(bytes: &[u8]) -> Option<(&str, &str)> {
    let v: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    if v.len() == 3 && v[2].is_empty() {
        Some((
            core::str::from_utf8(v[0]).ok()?,
            core::str::from_utf8(v[1]).ok()?,
        ))
    } else {
        None
    }
}
