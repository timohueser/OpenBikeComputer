//! One construction mechanism for the app's large resident components.
//!
//! Every KB-scale component has to be buildable two ways: hosts build it by value, while the
//! firmware must build it in place in the region the linker reserved for it, because a by-value
//! `App` is ~50 KB and the device stack is ~36 KB. Those two write mechanisms cannot be unified,
//! but the field plan behind them can. [`define_placement_constructors!`] generates both
//! constructors from one exhaustive plan, so a boot value can never be stated twice and drift.
//!
//! A plan row takes one of three forms: `field: expr`, written by both constructors from the same
//! expression; `field: expr => Type::init_fn`, a KB-scale field whose own placement constructor
//! writes it, with the by-value path still using `expr`; and `post |me| { … }`, one shared block of
//! safe mutation run by both constructors once every field exists, for state a `const` field
//! expression cannot state.
//!
//! # The placement invariant
//!
//! This is the crate's single safety contract for in-place construction; the generated functions
//! carry no separate wording, and neither should their call sites.
//!
//! A caller of a generated `unsafe fn init_*(slot, …)` must give a `slot` that is non-null,
//! aligned, writable and exclusively owned for a whole value of the type, and must not read it as
//! an initialized value beforehand. In return the function writes every field exactly once (the
//! emitted destructure fails to compile if the plan misses one) before it forms any reference to
//! the value, so the slot is fully initialized on return.
//!
//! That destructure proves only that every field was reached. For a `=> Type::init_fn` row the plan
//! author asserts that the named function satisfies this same invariant for that field's type, so a
//! new delegate must be macro-generated too, or carry a hand-checked proof that it writes its
//! target whole. A `post` block runs on an already-complete value, so it is ordinary safe code, but
//! no `post` block may be fallible: firmware construction does not unwind.

/// One plan row's placement write. It is split out only because a `macro_rules!` repetition cannot
/// branch on the optional `=> path` arm inline. The leading underscores mark it as
/// `define_placement_constructors!`'s expansion detail: it must be reachable crate-wide for that
/// macro's path to resolve, but it expands to a bare `unsafe` block, so nothing outside this file
/// may invoke it.
#[doc(hidden)]
macro_rules! __place_field {
    ($slot:ident, $field:ident, $value:expr => $init:path) => {
        unsafe { $init(core::ptr::addr_of_mut!((*$slot).$field)) }
    };
    ($slot:ident, $field:ident, $value:expr) => {
        unsafe { core::ptr::addr_of_mut!((*$slot).$field).write($value) }
    };
}

/// Generate a type's by-value and in-place constructors from one exhaustive field plan.
///
/// Invoked inside the type's own `impl` block. See the [module docs](self) for the row forms and
/// the safety contract the generated placement function relies on.
macro_rules! define_placement_constructors {
    (
        $(#[$new_meta:meta])*
        $new_vis:vis fn $new:ident($($arg:ident: $argty:ty),* $(,)?);
        $(#[$init_meta:meta])*
        $init_vis:vis unsafe fn $init:ident;
        fields {
            $( $(#[$field_meta:meta])* $field:ident: $value:expr $(=> $place:path)? ),+ $(,)?
        }
        $( post |$me:ident| $post:block )?
    ) => {
        $(#[$new_meta])*
        $new_vis fn $new($($arg: $argty),*) -> Self {
            #[allow(unused_mut)]
            let mut built = Self { $( $(#[$field_meta])* $field: $value, )+ };
            $( { let $me = &mut built; $post } )?
            built
        }

        $(#[$init_meta])*
        ///
        /// # Safety
        /// `slot` must satisfy the crate's placement invariant (`src/placement.rs`): non-null,
        /// aligned, writable and exclusively owned for a whole value of this type. On return the
        /// slot is fully initialized.
        $init_vis unsafe fn $init(slot: *mut Self $(, $arg: $argty)*) {
            $( $(#[$field_meta])* $crate::placement::__place_field!(slot, $field, $value $(=> $place)?); )+

            // Exhaustiveness guard: a field added to the type fails to compile here until the plan
            // above states it. It sits after every write, so forming the reference is sound.
            let Self { $( $(#[$field_meta])* $field: _, )+ } = unsafe { &*slot };

            $( { let $me = unsafe { &mut *slot }; $post } )?
        }
    };
}

pub(crate) use {__place_field, define_placement_constructors};
