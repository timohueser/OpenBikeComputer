//! Resident quadtree traversal shared by the host-side OBCM writers.
//!
//! This crate owns only the breadth-first node order and first-child numbering
//! required by the OBCM index. Tree construction, split/capacity policy, leaf
//! packing, streaming traversal, and format validation stay with their domains.

/// Enumerate `root` breadth-first and record each branch's first child.
///
/// `children` returns the four children in their wire order. They are appended
/// contiguously, so every branch's returned first-child index is greater than its
/// own index. Leaf entries retain the sentinel value `0`, which callers ignore.
#[inline]
pub fn breadth_first<'a, N>(
    root: &'a N,
    children: impl for<'n> Fn(&'n N) -> Option<&'n [N; 4]>,
) -> (Vec<&'a N>, Vec<usize>) {
    let mut nodes = vec![root];
    let mut first_child = vec![0];
    let mut index = 0;
    while index < nodes.len() {
        if let Some(children) = children(nodes[index]) {
            first_child[index] = nodes.len();
            for child in children {
                nodes.push(child);
                first_child.push(0);
            }
        }
        index += 1;
    }
    (nodes, first_child)
}

#[cfg(test)]
mod tests {
    use super::breadth_first;

    enum Node {
        Leaf(u8),
        Branch(Box<[Node; 4]>),
    }

    #[test]
    fn children_are_contiguous_in_breadth_first_order() {
        let root = Node::Branch(Box::new([
            Node::Leaf(0),
            Node::Branch(Box::new([Node::Leaf(1), Node::Leaf(2), Node::Leaf(3), Node::Leaf(4)])),
            Node::Leaf(5),
            Node::Leaf(6),
        ]));
        let (nodes, first_child) = breadth_first(&root, |node| match node {
            Node::Leaf(_) => None,
            Node::Branch(children) => Some(children),
        });
        assert_eq!(nodes.len(), 9);
        assert_eq!(first_child, [1, 0, 5, 0, 0, 0, 0, 0, 0]);
        let leaves: Vec<u8> = nodes
            .iter()
            .filter_map(|node| match node {
                Node::Leaf(value) => Some(*value),
                Node::Branch(_) => None,
            })
            .collect();
        assert_eq!(leaves, [0, 5, 6, 1, 2, 3, 4]);
    }
}
