//! The compile-time half of [`CardStore`](super::store::CardStore)'s lock law, as probes that fail.
//!
//! The law claims two things the type system enforces and one it does not, and prose is a poor
//! place to keep such a claim: the first version of it asserted that the compiler refuses to let a
//! repository view cross an `.await`, and the compiler did no such thing. So each half is written
//! here as a program, and each `compile_fail` probe is paired with a **sibling that must compile**
//! and differs from it in exactly the property under test.
//!
//! The pairing is the point. A `compile_fail` block passes when the snippet fails to build *for any
//! reason at all* — a renamed method, a moved module, a typo — so on its own it proves nothing. Its
//! sibling shares every path, every import and every call; if the module moves, the sibling stops
//! compiling and the suite goes red instead of quietly passing.
//!
//! These are doc tests, so they build against the crate as an external consumer sees it, and they
//! run under `cargo test --all-features` (the module is `std`-gated, so a feature-less doc build
//! collects none of them rather than passing them for the wrong reason).
//!
//! # Probe D — a repository view is not `Send`
//!
//! ```compile_fail
//! use obc_storage::obc2::card::Card;
//! use obc_storage::obc2::repositories::Routes;
//!
//! fn assert_send<T: Send>() {}
//! assert_send::<Routes<'static, Card>>();
//! ```
//!
//! The sibling: the store the view borrows *is* `Send`, so the failure above is the view's marker
//! and not something the media dragged in.
//!
//! ```
//! use obc_storage::obc2::card::Card;
//! use obc_storage::obc2::store::CardStore;
//!
//! fn assert_send<T: Send>() {}
//! assert_send::<CardStore<Card>>();
//! ```
//!
//! # Probe A — a future that holds a view across a suspension is not `Send`
//!
//! ```compile_fail
//! use obc_storage::obc2::card::Card;
//! use obc_storage::obc2::index::RamIndex;
//! use obc_storage::obc2::store::CardStore;
//! use obc_link::ids::StoreId;
//!
//! fn assert_send_future<F: core::future::Future + Send>(_: F) {}
//!
//! let (card, model) = Card::initialize(1, StoreId::new([7; 16]));
//! let mut store = Box::new(CardStore::mount(card, *RamIndex::project(&model), 0));
//! assert_send_future(async move {
//!     let routes = store.routes();
//!     core::future::pending::<()>().await;
//!     routes.count()
//! });
//! ```
//!
//! The sibling: the same future, with the view dropped **before** the suspension. That is the shape
//! the board glue keeps, and it is `Send`.
//!
//! ```
//! use obc_storage::obc2::card::Card;
//! use obc_storage::obc2::index::RamIndex;
//! use obc_storage::obc2::store::CardStore;
//! use obc_link::ids::StoreId;
//!
//! fn assert_send_future<F: core::future::Future + Send>(_: F) {}
//!
//! let (card, model) = Card::initialize(1, StoreId::new([7; 16]));
//! let mut store = Box::new(CardStore::mount(card, *RamIndex::project(&model), 0));
//! assert_send_future(async move {
//!     let routes = store.routes().count();
//!     core::future::pending::<()>().await;
//!     routes
//! });
//! ```
//!
//! Note what the pair does **not** say. Neither future is rejected by the compiler on its own; only
//! the `Send` bound separates them. A single-threaded executor imposes no such bound, so the first
//! future would run there — which is exactly why holding a view across an `.await` is a rule the
//! board glue keeps rather than one the compiler keeps for it.
//!
//! # Probe C — a view cannot be stashed past the call it was lent to
//!
//! ```compile_fail
//! use obc_storage::obc2::card::Card;
//! use obc_storage::obc2::index::RamIndex;
//! use obc_storage::obc2::store::CardStore;
//! use obc_link::ids::StoreId;
//!
//! let (card, model) = Card::initialize(1, StoreId::new([7; 16]));
//! let mut store = Box::new(CardStore::mount(card, *RamIndex::project(&model), 0));
//! let stashed = store.routes();
//! // The store is still borrowed by `stashed`, so nothing else may reach it.
//! let _ = store.head_count(obc_link::registry::ObjectKind::Route);
//! let _ = stashed.count();
//! ```
//!
//! The sibling: the same two calls, sequenced instead of overlapped.
//!
//! ```
//! use obc_storage::obc2::card::Card;
//! use obc_storage::obc2::index::RamIndex;
//! use obc_storage::obc2::store::CardStore;
//! use obc_link::ids::StoreId;
//!
//! let (card, model) = Card::initialize(1, StoreId::new([7; 16]));
//! let mut store = Box::new(CardStore::mount(card, *RamIndex::project(&model), 0));
//! let counted = store.routes().count();
//! let _ = store.head_count(obc_link::registry::ObjectKind::Route);
//! let _ = counted;
//! ```
//!
//! # Probe B — two views cannot exist at once
//!
//! ```compile_fail
//! use obc_storage::obc2::card::Card;
//! use obc_storage::obc2::index::RamIndex;
//! use obc_storage::obc2::store::CardStore;
//! use obc_link::ids::StoreId;
//!
//! let (card, model) = Card::initialize(1, StoreId::new([7; 16]));
//! let mut store = Box::new(CardStore::mount(card, *RamIndex::project(&model), 0));
//! let routes = store.routes();
//! let trips = store.trips();
//! let _ = (routes.count(), trips.count());
//! ```
//!
//! The sibling: one at a time, which is the whole of "lends … to one concrete repository at a time".
//!
//! ```
//! use obc_storage::obc2::card::Card;
//! use obc_storage::obc2::index::RamIndex;
//! use obc_storage::obc2::store::CardStore;
//! use obc_link::ids::StoreId;
//!
//! let (card, model) = Card::initialize(1, StoreId::new([7; 16]));
//! let mut store = Box::new(CardStore::mount(card, *RamIndex::project(&model), 0));
//! let routes = store.routes().count();
//! let trips = store.trips().count();
//! let _ = (routes, trips);
//! ```
