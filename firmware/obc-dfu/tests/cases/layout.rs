//! The bootloader's `memory.x` is static and committed, so nothing but this test holds it to
//! `obc_dfu::layout`. The board's linker map is generated from the same constants and cannot
//! drift on its own.

use obc_dfu::{APP_SLOT_BASE, APP_SLOT_LEN, BOOT_STATE_BASE, MAX_IMAGE_LEN, PAGE_LEN, SEMMC_STAGE_BASE, STAGE_LEN};

const MEMORY_X: &str = include_str!("../../../obc-boot/memory.x");

/// `ORIGIN` and `LENGTH` of one `MEMORY` region, in bytes.
fn region(name: &str) -> (u32, u32) {
    let line = MEMORY_X
        .lines()
        .find(|line| line.trim_start().starts_with(name) && line.contains("ORIGIN"))
        .unwrap_or_else(|| panic!("obc-boot/memory.x declares no `{name}` region"));
    (value(line, "ORIGIN = "), value(line, "LENGTH = "))
}

/// A region value as the linker spells it: `0x001F6000`, `32K` or a plain count.
fn value(line: &str, key: &str) -> u32 {
    let rest = line.split(key).nth(1).unwrap_or_else(|| panic!("`{key}` missing from `{line}`"));
    let token: String = rest.chars().take_while(char::is_ascii_alphanumeric).collect();
    if let Some(hex) = token.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).expect("a hexadecimal region value")
    } else if let Some(kib) = token.strip_suffix('K') {
        kib.parse::<u32>().expect("a K-suffixed region value") * 1024
    } else {
        token.parse().expect("a decimal region value")
    }
}

#[test]
fn the_app_slot_is_what_the_bootloader_links() {
    let (boot_base, boot_len) = region("FLASH");
    let (stage_base, stage_len) = region("SEMMC_STAGE");
    assert_eq!(
        APP_SLOT_BASE,
        boot_base + boot_len,
        "the app slot starts one past obc-boot's FLASH region; move both or neither"
    );
    assert_eq!(SEMMC_STAGE_BASE, stage_base, "the stage carve ends the app slot; move both or neither");
    assert_eq!(stage_len as usize, STAGE_LEN, "the carve's length is obc_dfu::blobstage::STAGE_LEN");
    let (state_base, state_len) = region("BOOT_STATE");
    assert_eq!(BOOT_STATE_BASE, state_base, "the armer writes the page the bootloader reads");
    assert_eq!(state_len as usize, PAGE_LEN, "the handoff page is one RRAM page");
    assert_eq!(SEMMC_STAGE_BASE + STAGE_LEN as u32, state_base, "the carve sits directly below the page");
    assert_eq!(APP_SLOT_LEN, stage_base - (boot_base + boot_len));
    assert_eq!(MAX_IMAGE_LEN, APP_SLOT_LEN, "the update path carries a whole slot, nothing less");
}
