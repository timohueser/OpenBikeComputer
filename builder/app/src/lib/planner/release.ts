/** The planner catalogue of specs/planner-release.md. The site reads it at page load, so a release needs no site build. */
export const PLANNER_CATALOG = "https://maps.openbikecomputer.com/planner/catalog.json";

/** The `active` entry of the planner catalogue. A catalogue format that this code does not know throws a message for the rider. */
export async function activeRelease(): Promise<Record<string, unknown>> {
    // Revalidate, so a page never keeps a browser copy beyond the catalogue's own cache lifetime.
    const response = await fetch(PLANNER_CATALOG, { cache: "no-cache" });
    if (!response.ok) throw new Error(`The map catalogue is unavailable (HTTP ${response.status}). Reload the page later.`);
    const catalog = await response.json();
    if (catalog?.format !== 1) throw new Error("The map catalogue has a format that this page does not know. Reload the page later.");
    if (!catalog.active || typeof catalog.active !== "object") throw new Error("The map catalogue has no active release.");
    return catalog.active;
}
