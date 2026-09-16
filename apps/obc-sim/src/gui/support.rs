use obc_app::device_core::PlatformSupport;

/// What the desktop simulator implements. Everything the shared screens can reach: the sim is the
/// device's development twin, and a capability withdrawn here would hide a screen the device has.
/// The bounded work behind DFU is simply never answered (the simulator platform), exactly as the old
/// command loop dropped that request — the headless `--png` path stages synthetic answers instead.
pub(crate) const SIM_SUPPORT: PlatformSupport =
    PlatformSupport { detour: true, settings_persistence: true, dfu: true, bonding: true, storage_space_report: true };
