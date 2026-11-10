//! The strongly connected components of a directed graph.

/// The strongly connected components of the graph whose node r leads to the nodes after[r], each
/// after every component that it leads to (Tarjan).
pub(crate) fn components(after: &[&[usize]]) -> Vec<Vec<usize>> {
    let n = after.len();
    let (mut index, mut low) = (vec![usize::MAX; n], vec![0; n]);
    let (mut stack, mut on, mut out) = (Vec::new(), vec![false; n], Vec::new());
    let mut counter = 0;
    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        index[root] = counter;
        low[root] = counter;
        counter += 1;
        stack.push(root);
        on[root] = true;
        let mut work = vec![(root, 0)];
        while let Some(&(node, position)) = work.last() {
            let mut next = position;
            let mut descended = false;
            while next < after[node].len() {
                let child = after[node][next];
                next += 1;
                if index[child] == usize::MAX {
                    index[child] = counter;
                    low[child] = counter;
                    counter += 1;
                    stack.push(child);
                    on[child] = true;
                    work.last_mut().unwrap().1 = next;
                    work.push((child, 0));
                    descended = true;
                    break;
                }
                if on[child] {
                    low[node] = low[node].min(index[child]);
                }
            }
            if descended {
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[node]);
            }
            if low[node] == index[node] {
                let mut component = Vec::new();
                while component.last() != Some(&node) {
                    let member = stack.pop().unwrap();
                    on[member] = false;
                    component.push(member);
                }
                out.push(component);
            }
        }
    }
    out
}
