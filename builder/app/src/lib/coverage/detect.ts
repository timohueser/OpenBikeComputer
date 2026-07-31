// Which catalog envelope a fetched root speaks (#1038).
//
// The hosted builder runs the v1 flow until the published catalog cuts over to
// the cell store, and the cutover is detected rather than deployed: one fetch of
// the root document, one peek at `schema_version`, and the app commits to the
// matching flow. The peek is deliberately *not* a parse — `parseCatalog` and
// `parseCatalogV2` each enforce a whole rulebook, and running one of them just to
// learn "wrong envelope" would report a malformed-v1 error for a perfectly good
// v2 document (or vice versa). The full parser for the detected version runs
// right after, on the same body, so nothing is fetched twice and nothing is
// admitted on the peek alone.

/**
 * The root's `schema_version`, or `null` when the body is not a JSON object
 * carrying a numeric one.
 *
 * `null` deliberately routes to the **v1** path: that parser owns the error
 * sentences for garbage ("expected an object", "not valid JSON…"), and a body
 * that is not JSON at all should fail with its diagnosis rather than with a
 * detection shrug.
 */
export function peekSchemaVersion(body: string): number | null {
    let root: unknown;
    try {
        root = JSON.parse(body);
    } catch {
        return null;
    }
    if (typeof root !== "object" || root === null || Array.isArray(root)) return null;
    const v = (root as Record<string, unknown>).schema_version;
    return typeof v === "number" ? v : null;
}
