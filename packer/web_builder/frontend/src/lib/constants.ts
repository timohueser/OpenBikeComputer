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
    import.meta.env.VITE_SITE_BASE || "https://timohueser.github.io/OpenBikeComputer/";

export const LINKS = {
    docs: `${SITE_BASE}docs/`,
    /** The landing page — which is where the live device demo lives (#624). */
    simulator: SITE_BASE,
    github: "https://github.com/timohueser/OpenBikeComputer",
    /** Where D3 (#908) publishes the desktop installers. */
    releases: "https://github.com/timohueser/OpenBikeComputer/releases",
};
