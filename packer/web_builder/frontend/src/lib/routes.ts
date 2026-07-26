// Route hashes as plain constants. Deliberately side-effect-free, unlike
// router.svelte.ts, which touches `window` at import.
//
// C2 (#901) owns this module and the `/desktop` page itself; the catalog picker
// links to it from every state that has to send someone to the desktop app, and
// does so through this constant rather than a literal.

export const DESKTOP_ROUTE = "#/desktop";
