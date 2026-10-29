// @vitest-environment happy-dom
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import areasExample from "../../../test-support/data/areas.json";
import review from "../../../test-support/data/prepare-review.json";
import layers from "../../../test-support/data/sources-layers.json";
import regionExample from "../../../test-support/data/sources-region.json";
import sources from "../../../test-support/data/sources.json";
import example from "../../../test-support/data/status.json";
import { CliQuery } from "../../lib/data/query.svelte";
import { screenAt } from "../../lib/data/screens";
import type { Status } from "../../lib/data/shapes";
import Sources from "./Sources.svelte";

const areas = {
    ...areasExample,
    regions: [
        ...areasExample.regions,
        { id: "central-switzerland", name: "Central Switzerland", countries: ["CH"], kind: "box", box: { west: 7.6, south: 46.46, east: 8.95, north: 47.25 } },
    ],
};

beforeEach(() => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date("2026-10-12T12:00:00Z"));
});

afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    document.body.replaceChildren();
    location.hash = "";
});

/** The region with a source that the planner can read after the stop of its step list. */
const region = {
    ...regionExample,
    sources: [
        ...regionExample.sources,
        {
            ...regionExample.sources.find((at) => at.source === "sentinel-2-mosaics")!,
            source: "hr-wsi",
            for_region: { need: "unknown", requests: 0, held_requests: 0, held_bytes: 0, download_bytes: 0, unknown_requests: 0, read_by: null, after_stop: true },
        },
    ],
};

/** The server answers the reads, a prepare review and a start; the page posts are kept. */
function screen(hash: string, answer: object = region) {
    const fetch = vi.fn(async (_url: string, init: { body: string }) => {
        const args: string[] = JSON.parse(init.body).args;
        const output =
            args[0] === "sources"
                ? args[1] === "--region"
                    ? answer
                    : args[1] === "--layer"
                      ? layers
                      : sources
                : args[0] === "areas"
                  ? areas
                  : args.includes("--dry-run")
                    ? review
                    : { run: "2026-10-12-120000", review: review.id };
        return new Response(JSON.stringify({ exit: 0, output }));
    });
    vi.stubGlobal("fetch", fetch);
    const status = new CliQuery<Status>([]);
    status.output = example as Status;
    const shown = mount(Sources, { target: document.body, props: { hash, status } });
    const posted = () => fetch.mock.calls.map(([, init]) => JSON.parse(init.body).args as string[]).filter((args) => !(["sources"].join() === args.join() || args[0] === "areas"));
    return { shown, posted };
}
const text = (at: Element = document.body) => at.textContent!.replace(/\s+/g, " ").trim();
/** The text of each child of `at`, such as the cells of a row, joined with spaces. */
const cells = (at: Element) => [...at.children].map((child) => text(child)).join(" ");
const button = (label: string) => [...document.querySelectorAll("button")].find((button) => button.textContent?.trim().startsWith(label))!;
const section = (title: string) => [...document.querySelectorAll("section")].find((at) => at.querySelector("h2")?.textContent === title)!;

it("the example status reads only sources of the example sources", () => {
    const targets = (example as Status).targets;
    const layers = new Set(targets.flatMap((target) => (target.layers ?? []).map((layer) => layer.layer)));
    const read = targets.flatMap((target) => [
        ...target.sources.map((at) => at.source),
        ...(target.layers ?? []).flatMap((layer) => [...layer.reads, ...(layer.cause?.source ? [layer.cause.source] : [])]),
    ]);
    const known = new Set(sources.sources.map((row) => row.id));
    expect(read.filter((id) => !layers.has(id) && !known.has(id))).toEqual([]);
});

it("lists every source by group with its status, what needs a hand first, and filters by status and text", async () => {
    const { shown } = screen("#/sources");
    try {
        await vi.waitFor(() => expect(document.querySelectorAll("details h2")).toHaveLength(6));
        expect([...document.querySelectorAll("details h2")].map((group) => group.textContent)).toEqual([
            "Prepared",
            "OpenStreetMap",
            "Elevation",
            "Climate and snow",
            "Land cover and imagery",
            "Map styles and tools",
        ]);
        expect([...document.querySelectorAll(".summary button")].map((at) => text(at))).toEqual(["Archived 2", "Prepared 4", "Held 1", "Partial 1", "Missing 1", "Live 4"]);
        const rows = () => [...document.querySelectorAll("tbody tr")];
        const osm = [...document.querySelectorAll("details")[1].querySelectorAll("tbody tr")];
        expect(text(osm[0]), "a stale source comes first, with why").toContain("Due again for this computer: 7 days old; the policy is 7 days");
        const glo = cells(rows().find((row) => row.textContent?.includes("copernicus-glo-30"))!);
        expect(glo).toBe("Terrain of the world at 30 m copernicus-glo-30 Partial 12 requests in the store World store 48 MB · 549 GB to get 2026-10-02 10 days old");
        const tool = rows().find((row) => row.textContent?.includes("planetiler"))!;
        expect(tool.querySelector<HTMLAnchorElement>('a[href="#/machine"]')?.textContent).toContain("Needs a login: GITHUB_TOKEN");
        expect(text(rows().find((row) => row.textContent?.includes("forest"))!)).toContain("A run stopped without freeing the lock.");

        button("Missing").click();
        flushSync();
        expect(rows().map((row) => row.querySelector("code")?.textContent)).toEqual(["canopy-ch"]);
        button("Live").click();
        flushSync();
        expect(rows()).toHaveLength(5);
        button("Missing").click();
        button("Live").click();
        const search = document.querySelector<HTMLInputElement>('input[type="search"]')!;
        search.value = "media/archive/prepared";
        search.dispatchEvent(new Event("input", { bubbles: true }));
        flushSync();
        expect(rows().map((row) => row.querySelector("code")?.textContent)).toEqual(["wiki", "forest", "terrain", "dtm-ch"]);
        expect(text(document.querySelector(".summary")!), "the counts follow the search").toContain("Prepared 4");
    } finally {
        await unmount(shown);
    }
});

it("a card gives the requests of a download the oldest first, and the command that fills its gap", async () => {
    const lines = async (id: string, first: string) => {
        const card = screen(`#/sources/${id}`);
        try {
            await vi.waitFor(() => expect(document.body.textContent).toContain(first));
            return [...section("On this machine").querySelectorAll("p, code")].map((at) => text(at));
        } finally {
            await unmount(card.shown);
        }
    };
    expect(await lines("geofabrik-poly", "Baden-Württemberg")).toEqual([
        "Live",
        "Rule · Read upstream when it is due",
        "Covers · World",
        "Where · store",
        "Size · 1000 B · Date 2026-10-01",
        "europe/germany/bayern · 2026-06-20 · 114 days old · 4.0 KB · older than its refresh of 90 days",
        "Baden-Württemberg · 2026-10-05 · 7 days old · 2.0 KB",
    ]);
    const glo = await lines("copernicus-glo-30", "4 tiles");
    expect(glo).toContain("4 tiles: N47_E007, N47_E008, N48_E007, … · 2021 · 41 days old · 1.5 GB");
    expect(glo.slice(-2)).toEqual(["Get the rest of its dataset into the archive.", "obc data archive get copernicus-glo-30"]);
    const canopy = screen("#/sources/canopy-ch");
    try {
        await vi.waitFor(() => expect(text()).toContain("The prepare of the source that reads it fetches it."));
    } finally {
        await unmount(canopy.shown);
    }
    const dtm = screen("#/sources/dtm-ch");
    try {
        await vi.waitFor(() => expect(text(section("Read by"))).toContain("The prepare of terrain; bakes read terrain."));
    } finally {
        await unmount(dtm.shown);
    }
});

it("a policy edit opens the Bake screen with its flag, and Refresh starts a run", async () => {
    const policy = screen("#/sources/geofabrik-extracts");
    try {
        await vi.waitFor(() => expect(button("Review This computer with it")).toBeDefined());
        expect(text(section("Read by"))).toContain("Layers · device/map, device/landmarks, planner/routing, planner/basemap, and planner/search");
        const input = document.querySelector<HTMLInputElement>("form input")!;
        const type = (value: string) => {
            input.value = value;
            input.dispatchEvent(new Event("input", { bubbles: true }));
            flushSync();
        };
        type("0");
        expect(button("Review This computer with it").disabled).toBe(true);
        type(" 14 ");
        button("Review This computer with it").click();
        expect(location.hash).toBe("#/bake?region=freiburg&on=device,map&refresh=geofabrik-extracts%3D14");
    } finally {
        await unmount(policy.shown);
    }
    const refresh = screen("#/sources/natural-earth");
    try {
        await vi.waitFor(() => expect(button("Refresh")).toBeDefined());
        button("Refresh").click();
        await vi.waitFor(() => expect(location.hash).toBe("#/runs/2026-10-12-120000"));
        expect(refresh.posted()).toEqual([["source", "refresh", "natural-earth"]]);
    } finally {
        await unmount(refresh.shown);
    }
});

it("a reference shows its versions over its models, a lock left behind, and the action that makes the next version", async () => {
    const { shown } = screen("#/sources/forest");
    try {
        await vi.waitFor(() => expect(text()).toContain("A run on laptop stopped without freeing the lock"));
        const versions = section("Versions");
        expect([...versions.querySelectorAll(".legend li")].map((li) => li.textContent)).toEqual(["In the archive", "National model, no version here yet"]);
        expect(text(versions)).toMatch(/2026-10-20 · [^·]+ · Central Switzerland/);
        const action = versions.querySelector<HTMLAnchorElement>("a.button")!;
        expect([action.textContent, action.getAttribute("href")]).toEqual(["Add an area", "#/sources/forest/prepare"]);
        expect(section("Refresh"), "a reference is prepared, not refreshed").toBeUndefined();
    } finally {
        await unmount(shown);
    }
});

it("the prepare link of the Bake screen opens the review of its area", async () => {
    const hash = "#/sources/forest/prepare?region=central-switzerland";
    expect(screenAt(hash).id).toBe("sources");
    const local = screen(hash);
    try {
        await vi.waitFor(() => expect(text()).toContain("Covers Central Switzerland"));
        expect(local.posted(), "a review waits for its button").toEqual([]);
        expect(text(), "a version of the forest adds an area, never all of the world").not.toContain("Worldwide");
        button("Review preparation").click();
        await vi.waitFor(() => expect(local.posted()).toEqual([["source", "prepare", "forest", "--region", "central-switzerland", "--dry-run"]]));
    } finally {
        await unmount(local.shown);
    }
});

it("a wiki review pins the region and languages, and changing them hides Start", async () => {
    const { shown, posted } = screen("#/sources/wiki/prepare?region=europe/dach");
    try {
        await vi.waitFor(() => expect(button("Review preparation")).toBeDefined());
        for (const code of ["fr", "es"]) document.querySelector<HTMLInputElement>(`input[value="${code}"]`)!.click();
        flushSync();
        button("Review preparation").click();
        await vi.waitFor(() => expect(text()).toContain(`Version ${review.version}`));
        expect(posted()).toEqual([["source", "prepare", "wiki", "--region", "europe/dach", "--languages", "en,de", "--dry-run"]]);
        expect(text()).toContain("Languages · English and German");
        const german = document.querySelector<HTMLInputElement>('input[value="de"]')!;
        german.click();
        flushSync();
        expect(button("Start")).toBeUndefined();
        german.click();
        flushSync();
        button("Start").click();
        await vi.waitFor(() => expect(location.hash).toBe("#/runs/2026-10-12-120000"));
        expect(posted().at(-1)).toEqual(review.args);
    } finally {
        await unmount(shown);
    }
});

it("by layer, each layer of `obc data sources --layer` has a chip for each source, with its status in words", async () => {
    const { shown, posted } = screen("#/sources?by=layer");
    try {
        await vi.waitFor(() => expect(document.querySelectorAll("section a.pill").length).toBe(5));
        expect(posted()).toEqual([["sources", "--layer"]]);
        expect([...section("search").querySelectorAll("a.pill")].map((chip) => `${chip.textContent} ${chip.getAttribute("data-status")}`)).toEqual([
            "osm-planet, Live live",
            "geofabrik-extracts, Live live",
            "wiki, Prepared prepared",
            "query-model, Prepared prepared",
            "nominatim-country-data, Held held",
        ]);
        expect(text(section("search"))).toContain("2 prepared · 1 held · 2 live");
    } finally {
        await unmount(shown);
    }
});

it("by region, it runs only the region listing, and gives the counters and what each source still needs", async () => {
    const { shown, posted } = screen("#/sources?by=region&region=grimsel");
    try {
        await vi.waitFor(() => expect(text()).toContain("Grimsel Pass · CH"));
        expect(posted()).toEqual([["sources", "--region", "grimsel"]]);
        expect([...document.querySelectorAll(".counters div")].map(cells)).toEqual([
            "Sources held 6 of 10",
            "Prepares needed 1 climate",
            "To download once 1 fetch of unknown size 1 source of unknown need",
            "OSM per update 524 MB",
        ]);
        expect(text(document.querySelector(".incomplete")!)).toMatch(/^planner is incomplete: its step list stops at the store lacks `sentinel-2-mosaics`/);
        expect([...section("Layers").querySelectorAll("tbody tr")].map(cells)[1]).toMatch(/^planner Unknown the store lacks `sentinel-2-mosaics`/);
        const row = (source: string) => cells([...section("Sources").querySelectorAll("tbody tr")].find((tr) => tr.querySelector("a")?.textContent === source)!);
        expect(row("copernicus-glo-30")).toBe("copernicus-glo-30 Held 2 fetches, 241 MB device/terrain —");
        expect(row("sentinel-2-mosaics")).toBe("sentinel-2-mosaics Download 0 of 1 fetches held — 1 fetch of unknown size");
        expect(row("era5-land")).toBe("era5-land Prepare read by climate — Prepare climate…");
        expect(row("hr-wsi"), "a source after the stop of its step list").toBe("hr-wsi Unknown after a missing input — —");
        expect([...document.querySelectorAll(".legend li")].map((li) => li.textContent), "terrain covers Grimsel with its version of Central Switzerland").toEqual([
            "The region",
            "A version that covers the region",
        ]);
    } finally {
        await unmount(shown);
    }
});

it("a region of several areas links its prepare with its own value, and an OSM size it cannot know is not 0 B", async () => {
    const osm = region.sources.map((at) =>
        at.source === "geofabrik-extracts" ? { ...at, for_region: { ...at.for_region, need: "download", held_requests: 0, held_bytes: 0, unknown_requests: 1 } } : at,
    );
    const areas = "europe/germany,europe/austria";
    const { shown, posted } = screen(`#/sources?by=region&region=${areas}`, { ...region, region: "areas", sources: osm });
    try {
        await vi.waitFor(() => expect(text()).toContain("OSM per update"));
        expect(posted()).toEqual([["sources", "--region", areas]]);
        const prepare = [...document.querySelectorAll<HTMLAnchorElement>("tbody a")].find((a) => a.textContent === "Prepare climate…")!;
        expect(prepare.getAttribute("href")).toBe(`#/sources/climate/prepare?region=${areas}`);
        const counter = (title: string) => cells([...document.querySelectorAll(".counters div")].find((at) => at.textContent?.startsWith(title))!);
        expect(counter("OSM per update")).toBe("OSM per update 1 fetch of unknown size");
        expect(counter("To download once"), "OSM is read again each update, not once").toBe("To download once 1 fetch of unknown size 1 source of unknown need");
    } finally {
        await unmount(shown);
    }
});
