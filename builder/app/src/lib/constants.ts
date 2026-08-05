/**
 * Links out of the builder, in one place.
 *
 * The docs and the landing page (with the live demo on it) are the other two
 * surfaces of the same site, and where they are depends on how *this* bundle was
 * deployed. `VITE_SITE_BASE` is that fact:
 *
 *   * unset — the default — means the site is somewhere else, so name it
 *     absolutely. That is the dev server and the desktop app: neither is served
 *     from the site, and a relative link there would point at nothing.
 *   * set to a relative prefix (the deploy passes `../`, because the builder is
 *     published at `/builder/` beside `/docs/`) means the site is right here.
 *     Nothing absolute is baked in, so the whole artifact survives the move to a
 *     custom domain — see .github/workflows/deploy-site.yml.
 *
 * GitHub is genuinely external and stays absolute in both.
 */
const SITE_BASE: string =
    import.meta.env.VITE_SITE_BASE || "https://openbikecomputer.com/";

export const LINKS = {
    docs: `${SITE_BASE}docs/`,
    /** The landing page — which is where the live device demo lives (#624). */
    simulator: SITE_BASE,
    /**
     * The legal pages, which § 5 DDG wants *ständig verfügbar* — from every page of
     * the offering, and this app is the part of it that stores things on the visitor's
     * device and talks to hardware. They live with the docs because that is the tree
     * that renders markdown; only the footer link is ours.
     */
    impressum: `${SITE_BASE}docs/impressum/`,
    datenschutz: `${SITE_BASE}docs/datenschutz/`,
    github: "https://github.com/timohueser/OpenBikeComputer",
    /**
     * The bundle's own third-party notices (#1149), emitted beside it at build time by
     * `vite/third-party-licenses.ts` — so it stays relative in every tier, including the
     * desktop app, where the file sits next to `index.html` inside the Tauri bundle.
     * `vite dev` has no bundle and therefore no file; the link 404s there by design,
     * because a stale checked-in copy would be worse than an absent one.
     */
    licenses: "./third-party-licenses.txt",
    /** Where D3 (#908) publishes the desktop installers. */
    releases: "https://github.com/timohueser/OpenBikeComputer/releases",
};
