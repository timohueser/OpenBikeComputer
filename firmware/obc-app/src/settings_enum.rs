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
            #[doc = concat!("This value's label in the UI `lang`, from the [`", stringify!($Name), "`] table.")]
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

            /// Walk `n` signed values through the ring, wrapping at both ends. The walk is
            /// arithmetic on the byte rather than a search through [`ALL`](Self::ALL): the
            /// discriminants are asserted contiguous below, so a value's byte is its index.
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
        // enum is a `settings_table!` row without a second declaration. Every declared enum gets
        // one, persisted or not; an unused one-byte impl is cheaper than a marker column that would
        // have to be kept true.
        $crate::settings_table::setting_enum_codec!($Name);

        // The on-disk contract, enforced: the table runs `0..COUNT` in declaration order, which is
        // what `ALL`, `cycled`, `stepped` and `from_byte` all assume. Renumber a row, reorder two,
        // or shift the range and the build stops, instead of a stored byte quietly decoding to a
        // different value.
        //
        // One loop and no per-variant assert: inside a macro, `$Var as u8 == $disc` compares a
        // value against the literal that declared it, so it cannot fail.
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
