//! One construction mechanism for the app's large resident components.
//!
//! Every KB-scale component ([`App`](crate::App) and the three it composes) has to be buildable two
//! ways: hosts build it **by value**, while the firmware must build it **in place** in the region
//! the linker reserved for it — a by-value `App` is ~50 KB and the device stack is ~36 KB. Those two
//! write mechanisms cannot be unified, but the *field plan* behind them can, and that is what
//! [`define_placement_constructors!`] does: one exhaustive plan per type generates both
//! constructors, so a boot value can never be stated twice and drift.
//!
//! A plan row takes one of three forms:
//!
//! - `field: expr` — a small field, written by both constructors from the same expression.
//! - `field: expr => Type::init_fn` — a KB-scale field whose own placement constructor writes it
//!   (the by-value path still uses `expr`).
//! - `post |me| { … }` — one shared block of safe mutation, run by both constructors once every
//!   field exists. It exists for state a `const` field expression cannot state, like the Home root
//!   on the screen stack (`heapless::Vec::push` is not `const`).
//!
//! # The placement invariant
//!
//! This is the crate's single safety contract for in-place construction; the generated functions
//! carry no separate wording, and neither should their call sites.
//!
//! A caller of a generated `unsafe fn init_*(slot, …)` must give a `slot` that is non-null,
//! aligned, writable and exclusively owned for a whole value of the type, and must not read it as
//! an initialized value beforehand. In return the function writes **every** field exactly once —
//! the emitted destructure fails to compile if the plan misses one — before it forms any reference
//! to the value, so the slot is fully initialized on return.
//!
//! A `post` block runs on an already-complete value, so it is ordinary safe code. If one panicked
//! the slot would be left initialized-but-unfinished; firmware construction does not unwind, and no
//! `post` block may be fallible.

/// One plan row's placement write. Split out because a `macro_rules!` repetition cannot branch on
/// the optional `=> path` arm inline. Not called directly — see [`define_placement_constructors!`].
macro_rules! place_field {
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
///
/// ```ignore
/// impl UiRuntime {
///     define_placement_constructors!(
///         /// Doc for the by-value constructor.
///         pub(crate) fn new();
///         /// Doc for the placement constructor.
///         pub(crate) unsafe fn init_in_place;
///         fields {
///             stack: Stack::new(),
///             cards: CardScheduler::new(),
///         }
///         post |ui| {
///             let _ = ui.stack.push(Screen::Home(HomeScreen::new()));
///         }
///     );
/// }
/// ```
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
            $( $(#[$field_meta])* $crate::placement::place_field!(slot, $field, $value $(=> $place)?); )+

            // Exhaustiveness guard: a field added to the type fails to compile here until the plan
            // above states it. No moves, no drops — it optimizes to nothing. It sits after every
            // write, so forming the reference is sound.
            let Self { $( $(#[$field_meta])* $field: _, )+ } = unsafe { &*slot };

            $( { let $me = unsafe { &mut *slot }; $post } )?
        }
    };
}

pub(crate) use {define_placement_constructors, place_field};
