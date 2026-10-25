// Hash routes, so that the page works behind any SSH tunnel port with no server rewrites.

export const SCREENS = [
    { id: "bake", label: "Bake", hash: "#/" },
    { id: "overview", label: "Overview", hash: "#/overview" },
    { id: "prod", label: "prod", hash: "#/prod" },
    { id: "local", label: "local", hash: "#/local" },
    { id: "downloads", label: "Downloads", hash: "#/downloads" },
    { id: "references", label: "References", hash: "#/references" },
    { id: "runs", label: "Runs", hash: "#/runs" },
    { id: "machine", label: "Machine", hash: "#/machine" },
] as const;

export type Screen = (typeof SCREENS)[number];

/** The screen of a hash such as `#/runs/ID`: its first segment names the screen. The Bake screen
 *  links a prepare as `#/sources/ID/prepare`, which References shows; Downloads shows the rest of
 *  `#/sources`. */
export function screenAt(hash: string): Screen {
    const first = hash.replace(/^#\/?/, "").split(/[/?]/)[0];
    const id = first === "sources" ? (hash.split("?")[0].endsWith("/prepare") ? "references" : "downloads") : first;
    return SCREENS.find((screen) => screen.id === id) ?? SCREENS[0];
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

/** What Downloads and References show: the list `#/downloads`, a source `#/downloads/ID`, or the
 *  review of a prepare `#/references/ID/prepare?to=TARGET&region=ID|box=W,S,E,N`. An id can hold
 *  `/`, so it is encoded. */
export function sourceAt(hash: string): {
    id: string | null;
    prepare: string | null;
    area: { flag: "--region" | "--box"; value: string } | null;
} {
    const [path, query = ""] = hash.replace(/^#\/?/, "").split("?");
    const [, id, prepare] = path.split("/");
    const params = new URLSearchParams(query);
    const [region, box] = [params.get("region"), params.get("box")];
    return {
        id: id ? decodeURIComponent(id) : null,
        prepare: prepare === "prepare" ? (params.get("to") ?? "prod") : null,
        area: region ? { flag: "--region", value: region } : box ? { flag: "--box", value: box } : null,
    };
}

export function prepareHash(id: string, to: string, area: { flag: "--region" | "--box"; value: string } | null = null): string {
    const at = area ? `&${area.flag === "--region" ? "region" : "box"}=${encodeURIComponent(area.value).replaceAll("%2F", "/").replaceAll("%2C", ",")}` : "";
    return `#/references/${encodeURIComponent(id)}/prepare?to=${to}${at}`;
}

/** What the Bake screen shows, from `#/bake?to=TARGET&region=ID|box=W,S,E,N&on=CARD,…&plan=RUN&run=RUN`:
 *  the URL keeps each choice, so that a reload or a link opens the same step. */
export interface BakeRoute {
    to: string;
    area: { flag: "--region" | "--box"; value: string } | null;
    on: string[];
    plan: string | null;
    run: string | null;
}

export function bakeAt(hash: string): BakeRoute {
    const params = new URLSearchParams(hash.split("?")[1] ?? "");
    const region = params.get("region");
    const box = params.get("box");
    return {
        to: params.get("to") ?? "local",
        area: region ? { flag: "--region", value: region } : box ? { flag: "--box", value: box } : null,
        on: params.get("on")?.split(",").filter(Boolean) ?? [],
        plan: params.get("plan"),
        run: params.get("run"),
    };
}

export function bakeHash({ to, area, on, plan, run }: BakeRoute): string {
    const params = new URLSearchParams();
    if (to !== "local") params.set("to", to);
    if (area) params.set(area.flag === "--region" ? "region" : "box", area.value);
    if (on.length) params.set("on", on.join(","));
    if (plan) params.set("plan", plan);
    if (run) params.set("run", run);
    const query = params.toString().replaceAll("%2C", ",").replaceAll("%2F", "/");
    return `#/bake${query ? `?${query}` : ""}`;
}
