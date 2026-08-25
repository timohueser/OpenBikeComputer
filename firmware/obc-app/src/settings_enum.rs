//! The one settings-value enum table.
//!
//! Every persisted settings *choice* — climb mode, waypoint chip, up-ahead source, idle return,
//! weather refresh, language, units — is the same five things restated: on-disk discriminants, a
//! default, a label, a picker walk, and a decode clamp. [`setting_enum!`] takes the declaration and
//! writes all five, so **the declaration is the contract**: the doc comments that promise
//! "appended, never renumbered" are now compile-time asserts, and a renumbered variant is a build
//! failure instead of a review miss.
//!
//! Deliberately a dumb token-pasting table, not a framework — the sibling of
//! [`screens!`](crate::screen) one module over. It generates transcription (`ALL`, `name`,
//! `cycled`, `stepped`, `from_byte`) and nothing else: predicates like
//! [`ClimbMode::is_on`](crate::settings::ClimbMode::is_on) are logic, and stay hand-written in a
//! plain `impl` block under their declaration.

/// Declare a settings value enum: its variants and their on-disk bytes, its default, and its
/// labels. Generates the enum (`#[repr(u8)]`, explicit discriminants), `Default`, `COUNT`, `ALL`,
/// `name`, `cycled`, `stepped`, `from_byte`, and the discriminant asserts.
///
/// One declaration order serves both roles it has to serve: it **is** the wire order (variant *i*
/// stores byte *i*, asserted below) and it **is** the picker order (`ALL`, which `cycled` and
/// `stepped` walk) — so there is no second `ORDER` list to keep in sync with the first.
///
/// ```ignore
/// setting_enum! {
///     /// How the Climb screen is reached.
///     pub enum ClimbMode {
///         /// The Climb screen is disabled.
///         Off = 0, key Msg::ClimbModeOff;
///         /// In the Back-cycle when a climb is active.
///         Manual = 1, key Msg::ClimbModeManual;
///         /// …and the device switches to it on climb entry.
///         Auto = 2, key Msg::ClimbModeAuto;
///     }
///     /// **Auto** out of the box — the climb panel is self-discovering.
///     default Auto;
/// }
/// ```
///
/// Three columns, one optional:
///
/// - `key Msg::…` labels through the i18n catalog → `name(self, lang)`. `text "Deutsch"` labels
///   with a literal → `name(self)`, for the one enum ([`Language`](crate::settings::Language))
///   whose labels are endonyms and so cannot be translated.
/// - A trailing per-variant expression plus a `payload name: Type;` line generates one accessor
///   (`IdleReturn::timeout_ms`, `WeatherRefresh::minutes`). Exactly one payload column per enum.
macro_rules! setting_enum {
    // Catalog-keyed labels, with a payload column.
    (
        $(#[$em:meta])*
        $vis:vis enum $Name:ident {
            $( $(#[$vm:meta])* $Var:ident = $disc:literal, key $key:expr, $pay:expr ; )+
        }
        $(#[$dm:meta])*
        default $Default:ident;
        $(#[$pm:meta])*
        payload $pfn:ident: $pty:ty;
    ) => {
        setting_enum!(@core
            $(#[$em])* $vis enum $Name { $( $(#[$vm])* $Var = $disc; )+ }
            $(#[$dm])* default $Default;
        );
        setting_enum!(@keyed $Name { $( $Var = $key; )+ });

        impl $Name {
            $(#[$pm])*
            #[inline]
            pub const fn $pfn(self) -> $pty {
                match self {
                    $( Self::$Var => $pay, )+
                }
            }
        }
    };

    // Catalog-keyed labels.
    (
        $(#[$em:meta])*
        $vis:vis enum $Name:ident {
            $( $(#[$vm:meta])* $Var:ident = $disc:literal, key $key:expr ; )+
        }
        $(#[$dm:meta])*
        default $Default:ident;
    ) => {
        setting_enum!(@core
            $(#[$em])* $vis enum $Name { $( $(#[$vm])* $Var = $disc; )+ }
            $(#[$dm])* default $Default;
        );
        setting_enum!(@keyed $Name { $( $Var = $key; )+ });
    };

    // Literal labels (untranslatable by nature — see `Language`).
    (
        $(#[$em:meta])*
        $vis:vis enum $Name:ident {
            $( $(#[$vm:meta])* $Var:ident = $disc:literal, text $txt:literal ; )+
        }
        $(#[$dm:meta])*
        default $Default:ident;
    ) => {
        setting_enum!(@core
            $(#[$em])* $vis enum $Name { $( $(#[$vm])* $Var = $disc; )+ }
            $(#[$dm])* default $Default;
        );

        impl $Name {
            #[doc = concat!("This value's label — a literal, not a catalog lookup (see [`", stringify!($Name), "`]).")]
            #[inline]
            pub const fn name(self) -> &'static str {
                match self {
                    $( Self::$Var => $txt, )+
                }
            }
        }
    };

    (@keyed $Name:ident { $( $Var:ident = $key:expr; )+ }) => {
        impl $Name {
            #[doc = concat!("This value's label in the UI `lang` (epic #602), from the [`", stringify!($Name), "`] table.")]
            #[inline]
            pub const fn name(self, lang: $crate::settings::Language) -> &'static str {
                match self {
                    $( Self::$Var => $crate::i18n::t($key, lang), )+
                }
            }
        }
    };

    (@core
        $(#[$em:meta])*
        $vis:vis enum $Name:ident {
            $( $(#[$vm:meta])* $Var:ident = $disc:literal; )+
        }
        $(#[$dm:meta])*
        default $Default:ident;
    ) => {
        $(#[$em])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[repr(u8)]
        $vis enum $Name {
            $( $(#[$vm])* $Var = $disc, )+
        }

        impl Default for $Name {
            $(#[$dm])*
            #[inline]
            fn default() -> Self {
                $Name::$Default
            }
        }

        impl $Name {
            #[doc = concat!("The number of [`", stringify!($Name), "`] values.")]
            pub const COUNT: usize = [$( Self::$Var, )+].len();

            #[doc = concat!("Every [`", stringify!($Name), "`] value, in table order — which is both the on-disk order and the picker's walk order.")]
            pub const ALL: [Self; Self::COUNT] = [$( Self::$Var, )+];

            /// The next value in the ring, wrapping — a press-to-cycle row's one action.
            #[inline]
            pub const fn cycled(self) -> Self {
                Self::from_byte(((self as usize + 1) % Self::COUNT) as u8)
            }

            /// Walk `n` values through the ring, wrapping at both ends — a value picker's
            /// left/right step (`n` is signed, and a multi-step flick compounds).
            ///
            /// The walk is arithmetic on the byte rather than a search through
            /// [`ALL`](Self::ALL) — the discriminants are asserted contiguous below, so a value's
            /// byte *is* its index in the ring.
            #[inline]
            pub fn stepped(self, n: i32) -> Self {
                Self::from_byte((self as i32 + n).rem_euclid(Self::COUNT as i32) as u8)
            }

            #[doc = concat!("Rebuild from a stored byte, sanitising an unknown value to [`", stringify!($Name), "::", stringify!($Default), "`] — the decode-side clamp every codec enum shares.")]
            #[inline]
            pub const fn from_byte(b: u8) -> Self {
                match b {
                    $( $disc => Self::$Var, )+
                    _ => Self::$Default,
                }
            }
        }

        // The settings-blob codec: the declared discriminant is the stored byte, so a declared
        // enum is a `settings_table!` row without a second declaration.
        //
        // Every declared enum gets this, persisted or not — the two sets coincide today, and one
        // unused one-byte impl is cheaper than a marker column that would have to be kept true. An
        // enum that is *not* persisted simply never appears in the table; nothing else changes.
        $crate::settings_table::setting_enum_codec!($Name);

        // The on-disk contract, enforced: the table still runs `0..COUNT` in declaration order,
        // which is exactly what `ALL`, `cycled`, `stepped`, and `from_byte` all assume. Renumber a
        // row, reorder two, or shift the range and the build stops, instead of a stored byte
        // quietly decoding to a different value.
        //
        // One loop and no per-variant assert: inside a macro, `$Var as u8 == $disc` compares a
        // value against the very literal that declared it, so it cannot fail. This is the whole
        // check.
        const _: () = {
            let mut i = 0;
            while i < $Name::COUNT {
                assert!(
                    $Name::ALL[i] as u8 == i as u8,
                    concat!(stringify!($Name), "'s discriminants must run 0..COUNT in table order"),
                );
                i += 1;
            }
        };
    };
}

pub(crate) use setting_enum;
