//! A display label, separate from checked producer and publication-owner identities.

pub(super) fn current() -> String {
    label(name().as_deref())
}

fn label(name: Option<&str>) -> String {
    let name = name.map(str::trim).filter(|name| !name.is_empty() && !name.chars().any(char::is_control));
    format!("{} ({})", name.unwrap_or("unknown host"), std::env::consts::OS)
}

#[cfg(unix)]
fn name() -> Option<String> {
    let mut bytes = [0u8; 256];
    // The fixed buffer is writable for its full length; a truncated result is not a label.
    if unsafe { libc::gethostname(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return None;
    }
    let end = bytes.iter().position(|byte| *byte == 0)?;
    String::from_utf8(bytes[..end].to_vec()).ok()
}

#[cfg(not(unix))]
fn name() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_distinguishes_hosts_and_keeps_an_explicit_unknown_fallback() {
        assert_ne!(label(Some("laptop")), label(Some("vps")));
        assert!(label(Some("laptop")).contains(std::env::consts::OS));
        for missing in [None, Some(""), Some("  "), Some("bad\x1bhost")] {
            assert!(label(missing).starts_with("unknown host ("));
        }
    }
}
