// @vitest-environment happy-dom
import { flushSync, mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import review from "../../../test-support/data/prepare-review.json";
import sources from "../../../test-support/data/sources.json";
import example from "../../../test-support/data/status.json";
import { CliQuery } from "../../lib/data/query.svelte";
import type { Status } from "../../lib/data/shapes";
import { family } from "../../lib/data/sources";
import Sources from "./Sources.svelte";

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
    location.hash = "";
});

/** The server answers `sources`, a prepare review and a start; the page posts are kept. */
function screen(hash: string, loaded = true) {
    const fetch = vi.fn(async (_url: string, init: { body: string }) => {
        const args: string[] = JSON.parse(init.body).args;
        const output = args[0] === "sources" ? sources : args.includes("--dry-run") ? review : { run: "2026-10-12-120000", review: review.id };
        return new Response(JSON.stringify({ exit: 0, output }));
    });
    vi.stubGlobal("fetch", fetch);
    const status = new CliQuery<Status>([]);
    if (loaded) status.output = example as Status;
    const shown = mount(Sources, { target: document.body, props: { hash, status } });
    const posted = () => fetch.mock.calls.map(([, init]) => JSON.parse(init.body).args as string[]).filter((args) => args[0] !== "sources");
    return { shown, posted, status };
}
const text = () => document.body.textContent ?? "";
const button = (label: string) => [...document.querySelectorAll("button")].find((button) => button.textContent?.trim() === label)!;

it("the example status reads only sources of the example sources", () => {
    const targets = (example as Status).targets;
    const layers = new Set(targets.flatMap((target) => (target.layers ?? []).map((layer) => layer.layer)));
    const read = targets.flatMap((target) => [
        ...target.sources.map((at) => at.source),
        ...(target.layers ?? []).flatMap((layer) => [...layer.reads, ...(layer.cause?.source ? [layer.cause.source] : [])]),
    ]);
    const known = new Set(sources.sources.flatMap((row) => [row.id, family(row.id)]));
    expect(read.filter((id) => !layers.has(id) && !known.has(id))).toEqual([]);
});

it("lists the sources by group, worst first, and folds a group that needs nothing", async () => {
    const { shown } = screen("#/sources");
    try {
        await vi.waitFor(() => expect(document.querySelectorAll("details")).toHaveLength(3));
        const groups = [...document.querySelectorAll("details")].map((group) => [group.querySelector("h2")!.textContent, group.open]);
        expect(groups).toEqual([
            ["OpenStreetMap", true],
            ["Elevation, Wikimedia, climate and snow", true],
            ["Basemap, land, assets and tools", false],
        ]);
        const first = [...document.querySelectorAll("details")[0].querySelectorAll("tbody td:first-child")].map((cell) => cell.textContent);
        expect(first).toEqual(["geofabrik-extracts", "osm-planet", "land-polygons"]);
        const family = [...document.querySelectorAll("tr")].find((row) => row.textContent?.startsWith("terrain-reference"))!;
        const cells = [...family.querySelectorAll("td")].map((cell) => cell.textContent);
        expect(cells, "a family counts the reads of its members").toEqual(["terrain-reference", "2 prepared", "local manual, prod manual", "1 layer", "ok"]);
    } finally {
        await unmount(shown);
    }
});

it("a policy edit opens the review with its flag, and Refresh starts a run", async () => {
    const policy = screen("#/sources/geofabrik-extracts");
    try {
        await vi.waitFor(() => expect(button("Review local with it")).toBeDefined());
        const input = document.querySelector<HTMLInputElement>("input")!;
        const type = (value: string) => {
            input.value = value;
            input.dispatchEvent(new Event("input", { bubbles: true }));
            flushSync();
        };
        type("0");
        expect(button("Review local with it").disabled).toBe(true);
        type(" 14 ");
        button("Review local with it").click();
        expect(location.hash).toBe(`#/local/review?refresh=${encodeURIComponent("geofabrik-extracts=14")}`);
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

it("shows the prepared versions of each target and the chips of a family", async () => {
    const wiki = screen("#/sources/wiki");
    try {
        await vi.waitFor(() => expect(text()).toContain("Prepared versions"));
        expect(text()).toMatch(/2026-06-01 · 2\.2 GB · complete [^·]+ · serves/);
        expect(button("Prepare a new version…")).toBeDefined();
    } finally {
        await unmount(wiki.shown);
    }
    const family = screen(`#/sources/${encodeURIComponent("terrain-reference")}`);
    try {
        await vi.waitFor(() => expect(document.querySelectorAll(".chip").length).toBe(4));
        const chips = [...document.querySelectorAll(".chip")].map((chip) => chip.textContent);
        expect(chips).toEqual(["ch · cannot be read", "de · not prepared", "ch · prepared 2026-08-20", "de · preparing on hetzner-bake-1"]);
    } finally {
        await unmount(family.shown);
    }
});

it("Start of a prepare posts exactly the args of its review, with --yes for an R2 target", async () => {
    const { shown, posted, status } = screen("#/sources/wiki/prepare?to=prod", false);
    try {
        await vi.waitFor(() => expect(text()).toContain(`Review ${review.id}`));
        expect(posted()).toEqual([["source", "prepare", "wiki", "--to", "prod", "--dry-run"]]);
        expect(text()).toContain("When it finishes · prod: device/landmarks and planner/search stale");
        expect(text()).toContain("Files · counted by the run · text published");
        expect(button("Start").disabled, "the kind of the target is not known yet").toBe(true);
        status.output = example as Status;
        flushSync();
        button("Start").click();
        await vi.waitFor(() => expect(location.hash).toBe("#/runs/2026-10-12-120000"));
        expect(posted().at(-1)).toEqual([...review.args, "--yes"]);
    } finally {
        await unmount(shown);
    }
});
