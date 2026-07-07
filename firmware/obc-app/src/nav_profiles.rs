//! The loaded map's routing-profile **names**, resident for the UI (routing-v2 N5, epic #533).
//!
//! The map's §8.6 profile table (names + the highway/surface multiplier arrays) lives host-side in
//! [`MapTables`](obc_reader::MapTables); the router reads it straight off the [`Reader`](obc_reader::Reader)
//! at plan time. The **UI**, though, needs the profile *names* in two places the reader isn't handed
//! to — the Bike-type settings screen (which cycles through them) and the created-route overview
//! label — and on the board those frames are drawn without a `Reader` at all (see
//! [`App::base_needs_reader`](crate::App::base_needs_reader)). So the App keeps this small resident
//! mirror of just the **names**, refreshed by the host whenever it (re)loads a map via
//! [`App::set_nav_profiles`](crate::App::set_nav_profiles) — the exact resident-catalog pattern the
//! route/ride catalogs already use ([`set_routes`](crate::App::set_routes)). The multiplier tables
//! are deliberately **not** duplicated here: they stay solely in `MapTables`, so the added resident
//! cost is only the names (≤ 8 × 12 B), never the routing weights.
//!
//! The selected profile is a bare index ([`Settings::bike_profile_idx`](crate::Settings)); an index
//! past the loaded map's profile count resolves to profile 0 at plan time (N3) and renders the
//! [`write_label`](NavProfiles::write_label) fallback (`Profile N`) so a stale device setting reads
//! honestly instead of showing a name the router won't use.

use core::fmt::Write;

use obc_reader::{MapProfile, NAV_MAX_PROFILES, NAV_PROFILE_NAME_LEN};

/// One profile name, sized to the §8.6 12-byte name field.
type ProfileName = heapless::String<NAV_PROFILE_NAME_LEN>;

/// The loaded map's routing-profile names, in table order. Empty before the first map load (and in
/// a router-less `ble` image, where the Bike-type screen still renders but is inert — the profile
/// names are map metadata, present regardless of whether the router is compiled in).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NavProfiles {
    names: heapless::Vec<ProfileName, NAV_MAX_PROFILES>,
}

impl NavProfiles {
    /// An empty set — no map loaded. `const`, so hosts and tests can hand out a `'static` borrow
    /// ([`Ctx`](crate::screen::Ctx) / [`Render`](crate::screen::Render) carry `&NavProfiles`).
    pub const EMPTY: NavProfiles = NavProfiles { names: heapless::Vec::new() };

    /// An empty set — see [`EMPTY`](NavProfiles::EMPTY).
    pub const fn new() -> Self {
        NavProfiles::EMPTY
    }

    /// Replace the resident names from the loaded map's parsed §8.6 profiles
    /// ([`Reader::nav_profiles`](obc_reader::Reader::nav_profiles)). Copies only the display names,
    /// truncated to the name field's byte cap on a char boundary (defensive — the parse already
    /// trims 0xFF padding). Called by [`App::set_nav_profiles`](crate::App::set_nav_profiles).
    pub fn set_from(&mut self, profiles: &[MapProfile]) {
        self.names.clear();
        for p in profiles.iter().take(NAV_MAX_PROFILES) {
            let mut name = ProfileName::new();
            // `push_str` fails only past the cap; a §8.6 name is ≤ 12 bytes, so this always fits.
            let _ = name.push_str(fit_name(p.name()));
            let _ = self.names.push(name);
        }
    }

    /// Build a set directly from display names — a convenience for hosts seeding a fixture and for
    /// tests, sidestepping the [`MapProfile`] multiplier arrays [`set_from`](NavProfiles::set_from)
    /// needs. Names past [`NAV_MAX_PROFILES`] or the name-byte cap are dropped / truncated, same as
    /// the map path.
    pub fn from_names(names: &[&str]) -> Self {
        let mut p = NavProfiles::new();
        for &n in names.iter().take(NAV_MAX_PROFILES) {
            let mut name = ProfileName::new();
            let _ = name.push_str(fit_name(n));
            let _ = p.names.push(name);
        }
        p
    }

    /// The number of profiles the loaded map carries (0 before any map load). The Bike-type screen
    /// cycles the index modulo this.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether no map profiles are resident yet — the inert state for the Bike-type screen.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The name of profile `idx`, or `None` if the index is past the loaded map's profile count (a
    /// stale device setting against a smaller map). Callers that want the display fallback use
    /// [`write_label`](NavProfiles::write_label).
    pub fn name(&self, idx: u8) -> Option<&str> {
        self.names.get(idx as usize).map(|s| s.as_str())
    }

    /// Write profile `idx`'s **display label** into `out`: the map's name when the index is in range
    /// and non-empty, else the generic `Profile N` fallback (`N` = the stored index) — so an
    /// out-of-range or unnamed profile still reads as something, never blank, and never a name the
    /// router won't actually use. The truthful-fallback rule of #538.
    pub fn write_label<const N: usize>(&self, idx: u8, out: &mut heapless::String<N>) {
        match self.name(idx) {
            Some(name) if !name.is_empty() => {
                let _ = out.push_str(name);
            }
            _ => {
                let _ = write!(out, "Profile {idx}");
            }
        }
    }
}

/// Truncate a name to the §8.6 byte cap on a char boundary (never mid-UTF-8).
fn fit_name(name: &str) -> &str {
    if name.len() <= NAV_PROFILE_NAME_LEN {
        return name;
    }
    let mut end = NAV_PROFILE_NAME_LEN;
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The display label: an in-range name renders verbatim; an out-of-range index renders the
    /// `Profile N` fallback (the truthful-fallback rule — the rider isn't shown a name routing won't
    /// use). Exercised without a map by driving `write_label` off a hand-built set.
    #[test]
    fn label_in_range_then_fallback() {
        let mut p = NavProfiles::new();
        assert!(p.is_empty(), "no map → empty, inert");
        // No map loaded: every index is the fallback.
        let mut buf: heapless::String<24> = heapless::String::new();
        p.write_label(0, &mut buf);
        assert_eq!(buf.as_str(), "Profile 0", "empty set → generic label");

        // Two named profiles: in-range renders the name, past-the-end renders the fallback.
        p.names.push(heapless::String::try_from("Road").unwrap()).unwrap();
        p.names.push(heapless::String::try_from("MTB").unwrap()).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p.name(0), Some("Road"));
        assert_eq!(p.name(1), Some("MTB"));
        assert_eq!(p.name(2), None, "past the profile count");

        for (idx, want) in [(0u8, "Road"), (1, "MTB"), (2, "Profile 2"), (9, "Profile 9")] {
            let mut b: heapless::String<24> = heapless::String::new();
            p.write_label(idx, &mut b);
            assert_eq!(b.as_str(), want, "idx {idx} label");
        }
    }
}
