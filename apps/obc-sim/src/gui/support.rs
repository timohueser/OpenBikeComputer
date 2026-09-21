use obc_app::device_core::PlatformSupport;

/// What the desktop simulator implements: everything the shared screens can reach, because a
/// capability withdrawn here would hide a screen the device has. The bounded work behind DFU is
/// never answered; the headless `--png` path stages synthetic answers instead.
pub(crate) const SIM_SUPPORT: PlatformSupport =
    PlatformSupport { detour: true, settings_persistence: true, dfu: true, bonding: true, storage_space_report: true };
