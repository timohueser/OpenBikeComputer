//! Compile-time translation catalog (epic #602, L2).
//!
//! The UI is authored in English literals; this module swaps them for a `(Msg, Language)`
//! lookup so a screen renders in `settings.language` with **no global/ambient state** — the
//! render path stays stateless (variant B). Language comes from `settings.language`, which
//! already flows to every draw site through `Render` / `Ctx`.
//!
//! [`Msg`] (one variant per key) and [`TABLE`] (`[[&str; 4]; N]` in flash `.rodata`) are
//! **generated** by `build.rs` from `i18n/{en,de,fr,es}.toml`; the build fails if any of the
//! four is missing or carrying an extra key. Because `TABLE` is `const`, the whole catalog
//! lives in flash — nothing lands in the 256 KB RAM budget.
//!
//! Lookup is a plain double index, so it is a `const fn`:
//! [`t(msg, lang)`](t) for a standalone call, and the [`Render::t`] / [`Ctx::t`] convenience
//! wrappers that read the language off the context a screen already holds.

use crate::screen::{Ctx, Render};
use crate::settings::Language;

// Pulls in `pub enum Msg { … }` and `pub const TABLE: [[&str; 4]; N] = [ … ];`.
include!(concat!(env!("OUT_DIR"), "/i18n_gen.rs"));

/// The translation for `m` in `lang`: `TABLE[m as usize][lang as usize]`. The `Language`
/// discriminants (En=0, De=1, Fr=2, Es=3) match TABLE's column order, and every `Msg` maps
/// to a populated row, so the index never panics.
#[inline]
pub const fn t(m: Msg, lang: Language) -> &'static str {
    TABLE[m as usize][lang as usize]
}

impl<'a, 'd> Render<'a, 'd> {
    /// The translation for `m` in the current UI language (`self.settings.language`) — the
    /// draw-time convenience over [`t`].
    #[inline]
    pub fn t(&self, m: Msg) -> &'static str {
        t(m, self.settings.language)
    }
}

impl Ctx<'_> {
    /// The translation for `m` in the current UI language (`self.settings.language`) — the
    /// input-handling convenience over [`t`] (a settings screen needs copy in `handle`, not
    /// just `draw`).
    #[inline]
    pub fn t(&self, m: Msg) -> &'static str {
        t(m, self.settings.language)
    }
}
