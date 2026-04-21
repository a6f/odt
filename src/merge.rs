//! Facilities for converting a parsed Devicetree Source file into a tree of nodes.

use crate::SourceNode;
use crate::error::{Scribe, SourceError};
use crate::label::{LabelMap, LabelResolver};
use crate::parse::TypedRuleExt;
use crate::parse::rules::*;
use crate::path::NodePath;
use std::collections::BTreeMap;

#[derive(Copy, Clone)]
pub enum NodeChange<'a> {
    TopNode(&'a TopNode<'a>),
    ChildNode(&'a ChildNode<'a>),
    TopDelNode(&'a TopDelNode<'a>),
    DelNode(&'a DelNode<'a>),
}

#[derive(Copy, Clone)]
pub enum PropChange<'a> {
    Prop(&'a Prop<'a>),
    DelProp(&'a DelProp<'a>),
    TopDelNode(&'a TopDelNode<'a>),
    DelNode(&'a DelNode<'a>),
}

impl<'a> NodeChange<'a> {
    pub fn span(&self) -> &pest::Span<'a> {
        match self {
            NodeChange::TopNode(x) => x.span(),
            NodeChange::ChildNode(x) => x.span(),
            NodeChange::TopDelNode(x) => x.span(),
            NodeChange::DelNode(x) => x.span(),
        }
    }
}

impl<'a> PropChange<'a> {
    pub fn span(&self) -> &pest::Span<'a> {
        match self {
            PropChange::Prop(x) => x.span(),
            PropChange::DelProp(x) => x.span(),
            PropChange::TopDelNode(x) => x.span(),
            PropChange::DelNode(x) => x.span(),
        }
    }
}

pub type NodeChanges<'a> = BTreeMap<NodePath, Vec<NodeChange<'a>>>;
pub type PropChanges<'a> = BTreeMap<NodePath, Vec<PropChange<'a>>>;

/// Transforms a parse tree into a tree of SourceNodes indexed by path.
///
/// Include directives are ignored; they should already have been substituted by
/// `parse_concat_with_includes()`.
///
/// This handles deletions
/// of nodes and properties, property overrides, and label assignments.  We may delete invalid
/// constructs without evaluating them.  For example, we accept
///   / { x = <(0 / 0)>; };
///   / { /delete-property/ x; };
/// while `dtc` does not.
pub fn merge<'i>(
    dts: &Dts<'i>,
    scribe: &mut Scribe,
) -> (SourceNode<'i>, LabelMap, NodeChanges<'i>, PropChanges<'i>) {
    merge_impl::<false>(dts, scribe)
}

/// Like [`merge`], but delete operations are recorded in `NodeChanges`/`PropChanges` without
/// actually removing nodes or properties from the returned tree.
pub fn merge_keep_deleted<'i>(
    dts: &Dts<'i>,
    scribe: &mut Scribe,
) -> (SourceNode<'i>, LabelMap, NodeChanges<'i>, PropChanges<'i>) {
    merge_impl::<true>(dts, scribe)
}

fn merge_impl<'i, const KEEP_DELETED: bool>(
    dts: &Dts<'i>,
    scribe: &mut Scribe,
) -> (SourceNode<'i>, LabelMap, NodeChanges<'i>, PropChanges<'i>) {
    let mut root = SourceNode::default();
    let mut node_labels = LabelMap::new();
    let mut node_changes = NodeChanges::new();
    let mut prop_changes = PropChanges::new();
    let rootpath = NodePath::root();
    for top_def in dts.top_def {
        match top_def {
            TopDef::Header(_) => (),      // ignored
            TopDef::Include(_) => (),     // already processed
            TopDef::Memreserve(_) => (),  // ignored
            TopDef::TopOmitNode(_) => (), // ignored
            TopDef::TopNode(topnode) => {
                let path = match topnode.top_node_name {
                    TopNodeName::NodeReference(noderef) => {
                        match LabelResolver(&node_labels, &root).resolve(&rootpath, noderef) {
                            Ok(path) => path,
                            Err(e) => {
                                scribe.err(e);
                                continue;
                            }
                        }
                    }
                    TopNodeName::RootNodeName(_) => NodePath::root(),
                };
                node_changes
                    .entry(path.clone())
                    .or_default()
                    .push(NodeChange::TopNode(topnode));
                let node = root.walk_mut(path.segments()).unwrap();
                for label in topnode.label {
                    if let Err(e) = add_label(&mut node_labels, label, node, &path) {
                        scribe.err(e);
                    }
                }
                let body = topnode.node_body;
                fill_source_node::<KEEP_DELETED>(
                    &mut node_labels,
                    &mut node_changes,
                    &mut prop_changes,
                    node,
                    &path,
                    body,
                    scribe,
                );
            }
            TopDef::TopDelNode(topdelnode) => {
                let noderef = topdelnode.node_reference;
                match LabelResolver(&node_labels, &root).resolve(&rootpath, noderef) {
                    Ok(path) => {
                        let Some(node) = root.walk_mut(path.segments()) else {
                            panic!("deleting nonexistent node {path}");
                        };
                        mark_deleted(
                            &mut node_changes,
                            &mut prop_changes,
                            node,
                            &path,
                            NodeChange::TopDelNode(topdelnode),
                            PropChange::TopDelNode(topdelnode),
                        );
                        if !KEEP_DELETED {
                            node_labels.retain(|_, p| !p.starts_with(&path));
                            if path.is_root() {
                                root = SourceNode::default();
                            } else {
                                let (parent, child) = (path.parent(), path.leaf());
                                let parent = root.walk_mut(parent.segments()).unwrap();
                                parent.remove_child(child);
                            }
                        }
                    }
                    Err(e) => scribe.err(e),
                }
            }
        }
    }
    (root, node_labels, node_changes, prop_changes)
}

fn mark_deleted<'a>(
    node_changes: &mut NodeChanges<'a>,
    prop_changes: &mut PropChanges<'a>,
    node: &SourceNode,
    path: &NodePath,
    node_cause: NodeChange<'a>,
    prop_cause: PropChange<'a>,
) {
    node_changes
        .entry(path.clone())
        .or_default()
        .push(node_cause);
    for (name, _) in node.properties() {
        prop_changes
            .entry(path.join(name))
            .or_default()
            .push(prop_cause);
    }
    for (name, child) in node.children() {
        mark_deleted(
            node_changes,
            prop_changes,
            child,
            &path.join(name),
            node_cause,
            prop_cause,
        );
    }
}

fn fill_source_node<'o, 'i: 'o, const KEEP_DELETED: bool>(
    node_labels: &mut LabelMap,
    node_changes: &mut NodeChanges<'o>,
    prop_changes: &mut PropChanges<'o>,
    node: &mut SourceNode<'o>,
    path: &NodePath,
    body: &'i NodeBody<'i>,
    scribe: &mut Scribe,
) {
    let mut names_used = std::collections::HashSet::new();
    for prop_def in body.node_contents.prop_def {
        match prop_def {
            PropDef::Prop(prop) => {
                let name = prop.prop_name.unescape_name();
                if !names_used.insert(name) {
                    // dtc rejects this only during the first definition of a node.
                    // However, it seems sensible to reopen nodes at non-top level,
                    // but likely mistaken to redefine a property within a scope.
                    scribe.warn(prop.prop_name.err("duplicate property"));
                }
                prop_changes
                    .entry(path.join(name))
                    .or_default()
                    .push(PropChange::Prop(prop));
                node.set_property(name, prop);
            }
            PropDef::DelProp(delprop) => {
                let name = delprop.prop_name.unescape_name();
                names_used.remove(name);
                if node.get_property(name).is_some() {
                    prop_changes
                        .entry(path.join(name))
                        .or_default()
                        .push(PropChange::DelProp(delprop));
                    if !KEEP_DELETED {
                        node.remove_property(name);
                    }
                }
            }
        }
    }
    for child_def in body.node_contents.child_def {
        match child_def {
            ChildDef::ChildNode(childnode) => {
                let name = childnode.node_name.unescape_name();
                let child_path = path.join(name);
                node_changes
                    .entry(child_path.clone())
                    .or_default()
                    .push(NodeChange::ChildNode(childnode));
                let child = node.add_child(name);
                for child_node_prefix in childnode.child_node_prefix {
                    if let ChildNodePrefix::Label(label) = child_node_prefix {
                        if let Err(e) = add_label(node_labels, label, child, &child_path) {
                            scribe.err(e);
                        }
                    }
                }
                let body = childnode.node_body;
                fill_source_node::<KEEP_DELETED>(
                    node_labels,
                    node_changes,
                    prop_changes,
                    child,
                    &child_path,
                    body,
                    scribe,
                );
            }
            ChildDef::DelNode(delnode) => {
                let name = delnode.node_name.unescape_name();
                let childpath = path.join(name);
                if let Some(child) = node.get_child(name) {
                    mark_deleted(
                        node_changes,
                        prop_changes,
                        child,
                        &childpath,
                        NodeChange::DelNode(delnode),
                        PropChange::DelNode(delnode),
                    );
                }
                if !KEEP_DELETED {
                    node.remove_child(name);
                    // TODO:  This is potentially quadratic.  Could use the labels in the removed node.
                    node_labels.retain(|_, p| !p.starts_with(&childpath));
                }
            }
        }
    }
}

trait UnescapeName<'a> {
    fn unescape_name(&self) -> &'a str;
}

impl<'a> UnescapeName<'a> for NodeName<'a> {
    fn unescape_name(&self) -> &'a str {
        let s = self.str();
        s.strip_prefix('\\').unwrap_or(s)
    }
}

impl<'a> UnescapeName<'a> for PropName<'a> {
    fn unescape_name(&self) -> &'a str {
        let s = self.str();
        s.strip_prefix('\\').unwrap_or(s)
    }
}

fn add_label(
    node_labels: &mut LabelMap,
    label: &Label,
    node: &mut SourceNode,
    path: &NodePath,
) -> Result<(), SourceError> {
    let s = label.str().strip_suffix(':').unwrap();
    if let Some(old) = node_labels.insert(s.into(), path.clone()) {
        // dtc permits duplicate labels during evaluation, as long as only one survives.
        // This is accepted:
        //   / {
        //     x: a { };
        //     x: b { };
        //   };
        //   /delete-node/ &x;
        // Unclear if we need to emulate this.
        if old != *path {
            return Err(label.err(format!("Duplicate label also on {old}")));
        }
    }
    node.add_label(s);
    Ok(())
}

#[test]
fn test_duplicate_property() {
    let source = include_str!("testdata/duplicate_property.dts");
    let arena = crate::Arena::new();
    let dts = crate::parse::parse_typed(source, &arena).unwrap();
    let mut scribe = Scribe::new(false);
    let _dts = merge(dts, &mut scribe);
    let (warnings, errors) = scribe.into_inner();
    assert!(
        errors.is_empty(),
        "expected only warnings, got errors:\n{errors:?}"
    );
    let [err] = &warnings[..] else {
        panic!("expected one warning, got {warnings:?}");
    };
    let message = format!("{err}");
    assert!(
        message.contains("duplicate property") && message.contains("this time it's an error"),
        "unexpected error:\n{message}"
    );
}
