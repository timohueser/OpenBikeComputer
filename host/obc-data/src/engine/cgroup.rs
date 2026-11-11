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

/// The memory of the cgroup of this process, or `None` without cgroup v2 or its memory controller.
pub fn memory() -> Option<Cgroup> {
    let membership = fs::read_to_string("/proc/self/cgroup").ok()?;
    read(Path::new("/sys/fs/cgroup"), &membership)
}

/// `membership` has the format of `/proc/self/cgroup`, and `root` is the mount point of cgroup v2.
pub fn read(root: &Path, membership: &str) -> Option<Cgroup> {
    let path = membership.lines().find_map(|line| line.strip_prefix("0::"))?;
    let folder = root.join(path.trim_start_matches('/'));
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
        assert_eq!(read(&root, "4:memory:/bake/step\n"), None, "cgroup v1 has no limit here");
        fs::remove_dir_all(&root).unwrap();
    }
}
