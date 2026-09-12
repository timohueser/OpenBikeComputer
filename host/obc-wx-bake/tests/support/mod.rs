use std::collections::BTreeMap;

/// Read a published directory store back as `key -> bytes`.
pub fn published_tree(dir: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    let mut tree = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(&path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let key = path.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
                tree.insert(key, std::fs::read(&path).unwrap());
            }
        }
    }
    tree
}
