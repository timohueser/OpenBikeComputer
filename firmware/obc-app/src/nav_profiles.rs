//! The loaded map's routing-profile names, resident for the UI.
//!
//! The map's profile table (the names plus the highway/surface multiplier arrays) lives host-side in
//! [`MapTables`](obc_reader::MapTables), and the router reads it straight off the
//! [`Reader`](obc_reader::Reader) at plan time. The UI needs the profile names in two places that
//! are drawn without a `Reader` at all: the create-route sheet's bike-type editor and the
//! created-route overview label. So the App keeps this small resident mirror of the names only,
//! refreshed by [`App::set_nav_profiles`](crate::App::set_nav_profiles) whenever the host loads a
//! map. The multiplier tables are deliberately not duplicated, so the added resident cost is the
//! names (≤ 8 × 12 B) and never the routing weights.
//!
//! The selected profile is a bare index ([`Settings::bike_profile_idx`](crate::Settings)). An index
//! past the loaded map's profile count resolves to profile 0 at plan time, and
//! [`write_label`](NavProfiles::write_label) renders profile 0's name for it, so a stale device
//! setting reads honestly instead of showing a name the router will not act on.

use core::fmt::Write;

use obc_formats::obcm::{NAV_MAX_PROFILES, NAV_PROFILE_NAME_LEN};
use obc_reader::MapProfile;

/// One profile name, sized to the map format's 12-byte name field.
type ProfileName = heapless::String<NAV_PROFILE_NAME_LEN>;

/// The loaded map's routing-profile names, in table order. Empty before the first map load, and in
/// a router-less `ble` image, where the bike-type row still renders but is inert.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NavProfiles {
    names: heapless::Vec<ProfileName, NAV_MAX_PROFILES>,
}

impl NavProfiles {
    /// An empty set — no map loaded. `const`, so hosts and tests can hand out a `'static` borrow
    /// ([`Ctx`](crate::screen::Ctx) / [`Render`](crate::screen::Render) carry `&NavProfiles`).
    pub const EMPTY: NavProfiles = NavProfiles { names: heapless::Vec::new() };

    pub const fn new() -> Self {
        NavProfiles::EMPTY
    }

    /// Replace the resident names from the loaded map's parsed profiles. Copies only the display
    /// names, truncated to the name field's byte cap on a char boundary.
    pub fn set_from(&mut self, profiles: &[MapProfile]) {
        self.names.clear();
        for p in profiles.iter().take(NAV_MAX_PROFILES) {
            let mut name = ProfileName::new();
            // `push_str` fails only past the cap; a profile name is ≤ 12 bytes, so this always fits.
            let _ = name.push_str(fit_name(p.name()));
            let _ = self.names.push(name);
        }
    }

    /// Build a set directly from display names, for hosts seeding a fixture and for tests. Names
    /// past [`NAV_MAX_PROFILES`] or the name-byte cap are dropped or truncated, as on the map path.
    pub fn from_names(names: &[&str]) -> Self {
        let mut p = NavProfiles::new();
        for &n in names.iter().take(NAV_MAX_PROFILES) {
            let mut name = ProfileName::new();
            let _ = name.push_str(fit_name(n));
            let _ = p.names.push(name);
        }
        p
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The name of profile `idx`, or `None` if the index is past the loaded map's profile count.
    /// Callers that want the display fallback use [`write_label`](NavProfiles::write_label).
    pub fn name(&self, idx: u8) -> Option<&str> {
        self.names.get(idx as usize).map(|s| s.as_str())
    }

    /// The profile index the router will actually route under for stored index `idx`: `idx` when in
    /// range, else `0`. It mirrors the router's own out-of-range fallback, so the UI and the search
    /// can never disagree about which profile is in effect. Only meaningful against a non-empty
    /// table.
    pub fn effective(&self, idx: u8) -> u8 {
        if (idx as usize) < self.names.len() {
            idx
        } else {
            0
        }
    }

    /// Write the display label for stored index `idx` into `out`, always naming the profile the
    /// router will actually use: that profile's name in range, and profile 0's name out of range
    /// against a non-empty table. The generic `Profile N` appears only when there is no name to
    /// show, which is an empty table or a blank stored name field.
    pub fn write_label<const N: usize>(&self, idx: u8, out: &mut heapless::String<N>) {
        if self.names.is_empty() {
            let _ = write!(out, "Profile {idx}");
            return;
        }
        let eff = self.effective(idx);
        match self.name(eff) {
            Some(name) if !name.is_empty() => {
                let _ = out.push_str(name);
            }
            _ => {
                let _ = write!(out, "Profile {eff}");
            }
        }
    }
}

/// Truncate a name to the format's byte cap on a char boundary, never mid-UTF-8.
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

    /// The display label always names the profile the router will actually use: that profile's name
    /// in range, profile 0's name out of range against a non-empty table, and the generic
    /// `Profile N` only when the table is empty.
    #[test]
    fn label_in_range_then_fallback() {
        let mut p = NavProfiles::new();
        assert!(p.is_empty(), "no map → empty, inert");
        // No map loaded: only here is the generic label shown (there is no profile-0 name to use).
        let mut buf: heapless::String<24> = heapless::String::new();
        p.write_label(0, &mut buf);
        assert_eq!(buf.as_str(), "Profile 0", "empty set → generic label");

        // Two named profiles: in-range renders the name; past-the-end renders profile 0's name,
        // exactly what the router routes under for that stored index.
        p.names.push(heapless::String::try_from("Road").unwrap()).unwrap();
        p.names.push(heapless::String::try_from("MTB").unwrap()).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p.name(0), Some("Road"));
        assert_eq!(p.name(1), Some("MTB"));
        assert_eq!(p.name(2), None, "past the profile count");
        assert_eq!(p.effective(1), 1, "in range → itself");
        assert_eq!(p.effective(2), 0, "out of range → the router's profile-0 fallback");

        for (idx, want) in [(0u8, "Road"), (1, "MTB"), (2, "Road"), (9, "Road")] {
            let mut b: heapless::String<24> = heapless::String::new();
            p.write_label(idx, &mut b);
            assert_eq!(b.as_str(), want, "idx {idx} label");
        }
    }
}
