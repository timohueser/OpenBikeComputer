//! Explicit host limits apply to bake workers, never to serving or final publication.

use std::path::Path;

pub const SLICE: &str = "obc-data-bake.slice";
const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Budget {
    pub cpu_percent: u64,
    pub memory_bytes: u64,
    pub minimum_free: u64,
    pub alert: String,
}

impl Budget {
    pub fn environment() -> Result<Self, String> {
        Self::read(|name| std::env::var(name).map_err(|_| format!("configure {name} in the operator environment")))
    }

    fn read(mut setting: impl FnMut(&str) -> Result<String, String>) -> Result<Self, String> {
        let mut positive = |name: &str, scale: u64| {
            setting(name)?
                .parse::<u64>()
                .ok()
                .filter(|n| *n > 0)
                .and_then(|n| n.checked_mul(scale))
                .ok_or_else(|| format!("{name} needs a positive whole number within the host limit"))
        };
        let cpu_percent = positive("OBC_BAKE_CPU_PERCENT", 1)?;
        let memory_bytes = positive("OBC_BAKE_MEMORY_MIB", MIB)?;
        let minimum_free = positive("OBC_BAKE_MIN_FREE_MIB", MIB)?;
        let alert = setting("OBC_BAKE_ALERT_UNIT")?;
        if !alert.ends_with(".service")
            || alert.starts_with("obc-data-")
            || !alert.bytes().all(|c| c.is_ascii_alphanumeric() || b"_.-@".contains(&c))
        {
            return Err("OBC_BAKE_ALERT_UNIT needs the configured operator alert service".into());
        }
        Ok(Self { cpu_percent, memory_bytes, minimum_free, alert })
    }

    pub fn unit(&self) -> String {
        format!(
            "[Unit]\nDescription=OpenBikeComputer bake budget\n[Slice]\nCPUQuota={}%\nMemoryMax={}\n",
            self.cpu_percent, self.memory_bytes
        )
    }

    pub fn verify_controllers(&self, directory: &Path) -> Result<(), String> {
        let quota = std::fs::read_to_string(directory.join("cpu.max"))
            .map_err(|e| format!("configure delegated cgroup-v2 CPU control for {SLICE}: {e}"))?;
        let quota: Vec<_> = quota.split_whitespace().map(str::parse::<u64>).collect();
        let memory = std::fs::read_to_string(directory.join("memory.max"))
            .map_err(|e| format!("configure delegated cgroup-v2 memory control for {SLICE}: {e}"))?;
        if !matches!(quota.as_slice(), [Ok(limit), Ok(period)] if *period > 0
            && u128::from(*limit) * 100 == u128::from(self.cpu_percent) * u128::from(*period))
            || memory.trim().parse::<u64>() != Ok(self.memory_bytes)
        {
            return Err("effective bake CPU/memory limits differ from the operator setup".into());
        }
        Ok(())
    }

    pub fn disk(&self, path: &Path, estimated: u64) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::ffi::OsStrExt;
            let existing = path.ancestors().find(|path| path.exists()).ok_or("disk preflight has no existing path")?;
            let name = std::ffi::CString::new(existing.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
            let mut status = std::mem::MaybeUninit::<libc::statvfs>::uninit();
            // SAFETY: the NUL-terminated path and writable output remain valid for this call.
            if unsafe { libc::statvfs(name.as_ptr(), status.as_mut_ptr()) } != 0 {
                return Err(format!("disk preflight: {}", std::io::Error::last_os_error()));
            }
            // SAFETY: statvfs initialized the output on success.
            let status = unsafe { status.assume_init() };
            let available = u128::from(status.f_bavail) * u128::from(status.f_frsize);
            self.check_space(available, estimated)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (path, estimated);
            Err("host bake budgets need the configured Linux systemd host".into())
        }
    }

    fn check_space(&self, available: u128, estimated: u64) -> Result<(), String> {
        if available < u128::from(self.minimum_free) + u128::from(estimated) {
            return Err(
                "disk preflight cannot keep the configured free-space reserve; release space before retrying".into()
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_and_effective_controllers_are_checked_and_disk_estimates_keep_the_reserve() {
        let budget = Budget::read(|name| {
            Ok(match name {
                "OBC_BAKE_CPU_PERCENT" => "125",
                "OBC_BAKE_MEMORY_MIB" => "2048",
                "OBC_BAKE_MIN_FREE_MIB" => "4096",
                "OBC_BAKE_ALERT_UNIT" => "operator-alert.service",
                _ => unreachable!(),
            }
            .into())
        })
        .unwrap();
        let scratch = crate::store::tests::Scratch::new("host-bake-budget");
        std::fs::write(scratch.0.join("cpu.max"), "125000 100000\n").unwrap();
        std::fs::write(scratch.0.join("memory.max"), budget.memory_bytes.to_string()).unwrap();
        budget.verify_controllers(&scratch.0).unwrap();
        std::fs::write(scratch.0.join("cpu.max"), "max 100000\n").unwrap();
        assert!(budget.verify_controllers(&scratch.0).is_err());
        budget.check_space(u128::from(budget.minimum_free) + 10, 10).unwrap();
        assert!(budget.check_space(u128::from(budget.minimum_free) + 9, 10).is_err());
        assert!(Budget::read(|_| Err("missing setup".into())).is_err());
        assert!(Budget::read(|_| Ok("0".into())).is_err());
        assert!(Budget::read(|name| Ok(
            if name == "OBC_BAKE_ALERT_UNIT" { "other\nExecStart=bad" } else { "1" }.into()
        ))
        .is_err());
    }
}
