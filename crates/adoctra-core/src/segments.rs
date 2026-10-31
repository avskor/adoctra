//! Writing nested output without recursion (user decision 2026-10-31).
//!
//! A renderer writes one node's own text into an [`Out`] (a `String`
//! through `Deref`) and marks each nested node with [`Out::node`] instead of
//! rendering it in place. [`expand`] then writes the parts in order on an
//! explicit stack, rendering each marked node when it is reached, so the
//! depth of the tree never deepens the call stack. Backends use it for their
//! block trees; it knows nothing of any output format.

/// One node's rendered text, with the nested nodes still to render.
pub struct Out<'a, T> {
    text: String,
    parts: Vec<Part<'a, T>>,
}

enum Part<'a, T> {
    Text(String),
    Node(&'a T),
}

impl<'a, T> Out<'a, T> {
    fn new() -> Self {
        Out {
            text: String::new(),
            parts: Vec::new(),
        }
    }

    /// Render `node` here, after this node's text so far.
    pub fn node(&mut self, node: &'a T) {
        if !self.text.is_empty() {
            self.parts.push(Part::Text(std::mem::take(&mut self.text)));
        }
        self.parts.push(Part::Node(node));
    }

    fn finish(mut self) -> std::vec::IntoIter<Part<'a, T>> {
        if !self.text.is_empty() {
            self.parts.push(Part::Text(self.text));
        }
        self.parts.into_iter()
    }
}

impl<T> std::ops::Deref for Out<'_, T> {
    type Target = String;

    fn deref(&self) -> &String {
        &self.text
    }
}

impl<T> std::ops::DerefMut for Out<'_, T> {
    fn deref_mut(&mut self) -> &mut String {
        &mut self.text
    }
}

/// Write `root` and its nested nodes into `out`: `render` writes one node,
/// marking its nested nodes.
pub fn expand<'a, T>(
    root: &'a T,
    out: &mut String,
    mut render: impl FnMut(&'a T, &mut Out<'a, T>),
) {
    let mut first = Out::new();
    render(root, &mut first);
    expand_parts(first, out, render);
}

/// Like [`expand`], with the root written by `first`.
pub fn expand_with<'a, T>(
    root: &'a T,
    out: &mut String,
    first: impl FnOnce(&'a T, &mut Out<'a, T>),
    render: impl FnMut(&'a T, &mut Out<'a, T>),
) {
    let mut written = Out::new();
    first(root, &mut written);
    expand_parts(written, out, render);
}

fn expand_parts<'a, T>(
    first: Out<'a, T>,
    out: &mut String,
    mut render: impl FnMut(&'a T, &mut Out<'a, T>),
) {
    let mut stack = vec![first.finish()];
    while let Some(parts) = stack.last_mut() {
        match parts.next() {
            Some(Part::Text(text)) => out.push_str(&text),
            Some(Part::Node(node)) => {
                let mut nested = Out::new();
                render(node, &mut nested);
                stack.push(nested.finish());
            }
            None => {
                stack.pop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(&'static str, Vec<Tree>);

    #[test]
    fn writes_nested_nodes_in_order_without_recursion() {
        let tree = Tree(
            "a",
            vec![
                Tree("b", vec![Tree("c", Vec::new())]),
                Tree("d", Vec::new()),
            ],
        );
        let mut out = String::new();
        expand(&tree, &mut out, |node, s| {
            s.push_str(&format!("<{}>", node.0));
            for child in &node.1 {
                s.node(child);
            }
            s.push_str(&format!("</{}>", node.0));
        });
        assert_eq!(out, "<a><b><c></c></b><d></d></a>");

        // A chain far deeper than a recursive renderer's stack would allow.
        let mut deep = Tree("x", Vec::new());
        for _ in 0..100_000 {
            deep = Tree("x", vec![deep]);
        }
        let mut out = String::new();
        expand(&deep, &mut out, |node, s| {
            s.push('(');
            for child in &node.1 {
                s.node(child);
            }
            s.push(')');
        });
        assert_eq!(out.len(), 2 * 100_001);
        // The test tree drops iteratively too.
        let mut stack = vec![deep];
        while let Some(mut node) = stack.pop() {
            stack.append(&mut node.1);
        }
    }
}
