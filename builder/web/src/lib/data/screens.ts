// Hash routes, so that the page works behind any SSH tunnel port with no server rewrites.

export const SCREENS = [
    { id: "overview", label: "Overview", hash: "#/" },
    { id: "prod", label: "prod", hash: "#/prod" },
    { id: "local", label: "local", hash: "#/local" },
    { id: "sources", label: "Sources", hash: "#/sources" },
    { id: "runs", label: "Runs", hash: "#/runs" },
    { id: "machine", label: "Machine", hash: "#/machine" },
] as const;

export type Screen = (typeof SCREENS)[number];

/** The screen of a hash such as `#/runs/ID`: its first segment names the screen. */
export function screenAt(hash: string): Screen {
    const first = hash.replace(/^#\/?/, "").split("/")[0];
    return SCREENS.find((screen) => screen.id === first) ?? SCREENS[0];
}

/** The run of a hash such as `#/runs/ID`, or null on the list of runs. */
export function runAt(hash: string): string | null {
    const [first, id] = hash.replace(/^#\/?/, "").split("/");
    return first === "runs" && id ? id : null;
}

/** What a target screen shows: `#/TARGET`; its review `#/TARGET/review`, which `?only=LAYER,…` and
 *  `?refresh=SOURCE=DAYS` set; or the review that resumes a run, `#/TARGET/resume/RUN`. */
export function targetAt(hash: string): {
    review: boolean;
    resume: string | null;
    only: string | null;
    refresh: string | null;
    plan: string | null;
} {
    const [path, query = ""] = hash.replace(/^#\/?/, "").split("?");
    const [, second, run] = path.split("/");
    const resume = second === "resume" && run ? run : null;
    const params = new URLSearchParams(query);
    return {
        review: second === "review" || !!resume,
        resume,
        only: params.get("only"),
        refresh: params.get("refresh"),
        plan: params.get("plan"),
    };
}

/** What the Sources screen shows: the list `#/sources`, a source `#/sources/ID`, or the review of a
 *  prepare `#/sources/ID/prepare?to=TARGET`. An id can hold `/`, so it is encoded. */
export function sourceAt(hash: string): { id: string | null; prepare: string | null } {
    const [path, query = ""] = hash.replace(/^#\/?/, "").split("?");
    const [, id, prepare] = path.split("/");
    return { id: id ? decodeURIComponent(id) : null, prepare: prepare === "prepare" ? (new URLSearchParams(query).get("to") ?? "prod") : null };
}
