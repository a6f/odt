//! Facilities for converting a parsed Devicetree Source file into a tree of nodes.

use crate::SourceNode;
use crate::error::{Scribe, SourceError};
use crate::label::{LabelMap, LabelResolver};
use crate::parse::TypedRuleExt;
use crate::parse::rules::*;
use crate::path::NodePath;

#[derive(Copy, Clone)]
pub enum NodeCreate<'i> {
    TopNode(&'i TopNode<'i>),
    ChildNode(&'i ChildNode<'i>),
}

impl<'i> NodeCreate<'i> {
    pub fn span(&self) -> &pest::Span<'i> {
        match self {
            NodeCreate::TopNode(x) => x.span(),
            NodeCreate::ChildNode(x) => x.span(),
        }
    }
}

impl core::fmt::Debug for NodeCreate<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("...")
    }
}

#[derive(Copy, Clone, Debug)]
pub enum NodeDelete<'i> {
    TopDelNode(&'i TopDelNode<'i>),
    DelNode(&'i DelNode<'i>),
}

impl<'i> NodeDelete<'i> {
    pub fn span(&self) -> &pest::Span<'i> {
        match self {
            NodeDelete::TopDelNode(x) => x.span(),
            NodeDelete::DelNode(x) => x.span(),
        }
    }
}

/// Reports an operation performed during tree merging of parsed source code.
/// Lifetime 'e is for one event callback; lifetime 'i is for the source tree.
#[derive(Copy, Clone, Debug)]
pub enum MergeEvent<'e, 'i> {
    AddNode {
        path: &'e NodePath,
        create: NodeCreate<'i>,
    },
    DelNode {
        path: &'e NodePath,
        delete: NodeDelete<'i>,
    },
    AddProp {
        path: &'e NodePath,
        name: &'e str,
        prop: &'i Prop<'i>,
    },
    DelProp {
        path: &'e NodePath,
        name: &'e str,
        delprop: &'i DelProp<'i>,
    },
    AddLabel {
        path: &'e NodePath,
        labelname: &'e str,
        label: &'i Label<'i>,
    },
}

/// Transforms a parse tree into a tree of SourceNodes indexed by path.
///
/// Include directives are ignored; they should already have been substituted by
/// `parse_concat_with_includes()`.
///
/// This handles deletions of nodes and properties, property overrides, and label assignments.
/// We may delete invalid constructs without evaluating them.  For example, we accept
///   / { x = <(0 / 0)>; };
///   / { /delete-property/ x; };
/// while `dtc` does not.
pub fn merge<'i>(dts: &Dts<'i>, scribe: &mut Scribe) -> (SourceNode<'i>, LabelMap) {
    merge_with_events(dts, scribe, &mut |_| {})
}

/// Like `merge()`, but `events` is called with each input grammar node immediately before it is
/// processed.
pub fn merge_with_events<'i>(
    dts: &Dts<'i>,
    scribe: &mut Scribe,
    events: &mut impl FnMut(MergeEvent<'_, 'i>),
) -> (SourceNode<'i>, LabelMap) {
    let mut root = SourceNode::default();
    let mut node_labels = LabelMap::new();
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
                events(MergeEvent::AddNode {
                    path: &path,
                    create: NodeCreate::TopNode(topnode),
                });
                let node = root.walk_mut(path.segments()).unwrap();
                for label in topnode.label {
                    if let Err(e) = add_label(&mut node_labels, label, node, &path, events) {
                        scribe.err(e);
                    }
                }
                let body = topnode.node_body;
                fill_source_node(&mut node_labels, node, &path, body, scribe, events);
            }
            TopDef::TopDelNode(topdelnode) => {
                let noderef = topdelnode.node_reference;
                match LabelResolver(&node_labels, &root).resolve(&rootpath, noderef) {
                    Ok(path) => {
                        let Some(_node) = root.walk_mut(path.segments()) else {
                            panic!("deleting nonexistent node {path}");
                        };
                        events(MergeEvent::DelNode {
                            path: &path,
                            delete: NodeDelete::TopDelNode(topdelnode),
                        });
                        if path.is_root() {
                            root = SourceNode::default();
                            node_labels.clear();
                        } else {
                            let (parent, child) = (path.parent(), path.leaf());
                            let parent = root.walk_mut(parent.segments()).unwrap();
                            let mut node = parent.remove_child(child).unwrap();
                            node.visit_preorder_mut(&mut |n| {
                                for l in n.labels() {
                                    node_labels.remove(l);
                                }
                            });
                        }
                    }
                    Err(e) => scribe.err(e),
                }
            }
        }
    }
    (root, node_labels)
}

fn fill_source_node<'o, 'i: 'o>(
    node_labels: &mut LabelMap,
    node: &mut SourceNode<'o>,
    path: &NodePath,
    body: &'i NodeBody<'i>,
    scribe: &mut Scribe,
    events: &mut impl FnMut(MergeEvent<'_, 'i>),
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
                events(MergeEvent::AddProp { path, name, prop });
                node.set_property(name, prop);
            }
            PropDef::DelProp(delprop) => {
                let name = delprop.prop_name.unescape_name();
                names_used.remove(name);
                events(MergeEvent::DelProp {
                    path,
                    name,
                    delprop,
                });
                node.remove_property(name);
            }
        }
    }
    for child_def in body.node_contents.child_def {
        match child_def {
            ChildDef::ChildNode(childnode) => {
                let name = childnode.node_name.unescape_name();
                let child_path = path.join(name);
                events(MergeEvent::AddNode {
                    path: &child_path,
                    create: NodeCreate::ChildNode(childnode),
                });
                let child = node.add_child(name);
                for child_node_prefix in childnode.child_node_prefix {
                    if let ChildNodePrefix::Label(label) = child_node_prefix {
                        if let Err(e) = add_label(node_labels, label, child, &child_path, events) {
                            scribe.err(e);
                        }
                    }
                }
                let body = childnode.node_body;
                fill_source_node(node_labels, child, &child_path, body, scribe, events);
            }
            ChildDef::DelNode(delnode) => {
                let name = delnode.node_name.unescape_name();
                let child_path = path.join(name);
                events(MergeEvent::DelNode {
                    path: &child_path,
                    delete: NodeDelete::DelNode(delnode),
                });
                if let Some(mut child) = node.remove_child(name) {
                    child.visit_preorder_mut(&mut |n| {
                        for l in n.labels() {
                            node_labels.remove(l);
                        }
                    });
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

fn add_label<'i>(
    node_labels: &mut LabelMap,
    label: &'i Label<'i>,
    node: &mut SourceNode,
    path: &NodePath,
    events: &mut impl FnMut(MergeEvent<'_, 'i>),
) -> Result<(), SourceError> {
    let labelname = label.str().strip_suffix(':').unwrap();
    if let Some(old) = node_labels.insert(labelname.into(), path.clone()) {
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
    events(MergeEvent::AddLabel {
        path,
        labelname,
        label,
    });
    node.add_label(labelname);
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
