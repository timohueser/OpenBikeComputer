//! The memory limit and the memory use of the cgroup of this process, from the files of cgroup v2.

use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cgroup {
    /// `memory.max` in bytes, or `None` for no limit.
    pub max: Option<u64>,
    /// The anonymous memory of all processes of the cgroup: `anon` of `memory.stat`, in bytes.
    pub anon: u64,
}

/// The memory of the cgroup of this process or of one of its parents: the one with the least free
/// memory, `memory.max` less `anon`. `None` without cgroup v2 or its memory controller.
pub fn memory() -> Option<Cgroup> {
    let membership = fs::read_to_string("/proc/self/cgroup").ok()?;
    read(Path::new("/sys/fs/cgroup"), &membership)
}

/// The least `memory.max` of the cgroup of this process and of its parents: the limit that binds
/// whatever is in use. `None` for no limit, and without cgroup v2 or its memory controller.
pub fn limit() -> Option<u64> {
    let membership = fs::read_to_string("/proc/self/cgroup").ok()?;
    limit_in(Path::new("/sys/fs/cgroup"), &membership)
}

pub fn read(root: &Path, membership: &str) -> Option<Cgroup> {
    let free = |cgroup: &Cgroup| cgroup.max.map_or(u64::MAX, |max| max.saturating_sub(cgroup.anon));
    chain(root, membership)?.into_iter().min_by_key(free)
}

pub fn limit_in(root: &Path, membership: &str) -> Option<u64> {
    chain(root, membership)?.into_iter().filter_map(|cgroup| cgroup.max).min()
}

/// The cgroups from that of `membership` up to `root`. `membership` has the format of
/// `/proc/self/cgroup`, and `root` is the mount point of cgroup v2. A parent can set a limit that
/// its child does not show.
fn chain(root: &Path, membership: &str) -> Option<Vec<Cgroup>> {
    let path = membership.lines().find_map(|line| line.strip_prefix("0::"))?;
    let folder = root.join(path.trim_start_matches('/'));
    Some(folder.ancestors().take_while(|folder| folder.starts_with(root)).filter_map(at).collect())
}

/// The memory of the cgroup in `folder`; the root cgroup has no `memory.max`.
fn at(folder: &Path) -> Option<Cgroup> {
    let max = match fs::read_to_string(folder.join("memory.max")).ok()?.trim() {
        "max" => None,
        value => Some(value.parse().ok()?),
    };
    let stat = fs::read_to_string(folder.join("memory.stat")).ok()?;
    let anon = stat.lines().find_map(|line| line.strip_prefix("anon "))?.trim().parse().ok()?;
    Some(Cgroup { max, anon })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cgroup_limit_is_read_from_a_fake_tree() {
        let root = std::env::temp_dir().join(format!("obc-data-cgroup-{}", std::process::id()));
        let folder = root.join("bake/step");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("memory.max"), "1073741824\n").unwrap();
        fs::write(folder.join("memory.stat"), "file 7\nanon 4096\nanon_thp 0\n").unwrap();
        let membership = "1:name=systemd:/\n0::/bake/step\n";
        assert_eq!(read(&root, membership), Some(Cgroup { max: Some(1 << 30), anon: 4096 }));
        fs::write(folder.join("memory.max"), "max\n").unwrap();
        assert_eq!(read(&root, membership), Some(Cgroup { max: None, anon: 4096 }));
        // A parent's limit binds a child without a limit of its own.
        let parent = root.join("bake");
        fs::write(parent.join("memory.max"), "12884901888\n").unwrap();
        fs::write(parent.join("memory.stat"), "anon 8192\n").unwrap();
        assert_eq!(read(&root, membership), Some(Cgroup { max: Some(12 << 30), anon: 8192 }));
        assert_eq!(read(&root, "4:memory:/bake/step\n"), None, "cgroup v1 has no limit here");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_limit_is_the_least_max_of_the_chain_whatever_is_in_use() {
        let root = std::env::temp_dir().join(format!("obc-data-cgroup-limit-{}", std::process::id()));
        let (parent, folder) = (root.join("bake"), root.join("bake/step"));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("memory.max"), "4294967296\n").unwrap();
        fs::write(folder.join("memory.stat"), "anon 536870912\n").unwrap();
        fs::write(parent.join("memory.max"), "8589934592\n").unwrap();
        fs::write(parent.join("memory.stat"), "anon 6442450944\n").unwrap();
        let membership = "0::/bake/step\n";
        assert_eq!(read(&root, membership).unwrap().max, Some(8 << 30), "the parent has the least free memory");
        assert_eq!(limit_in(&root, membership), Some(4 << 30));
        fs::write(folder.join("memory.max"), "max\n").unwrap();
        assert_eq!(limit_in(&root, membership), Some(8 << 30));
        fs::write(parent.join("memory.max"), "max\n").unwrap();
        assert_eq!(limit_in(&root, membership), None);
        fs::remove_dir_all(&root).unwrap();
    }
}
