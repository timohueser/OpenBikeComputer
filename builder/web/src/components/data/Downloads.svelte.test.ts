// @vitest-environment happy-dom
import { flushSync, mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import sources from "../../../test-support/data/sources.json";
import example from "../../../test-support/data/status.json";
import { CliQuery } from "../../lib/data/query.svelte";
import type { Status } from "../../lib/data/shapes";
import { family } from "../../lib/data/sources";
import Downloads from "./Downloads.svelte";

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
    location.hash = "";
});

/** The server answers `sources` and a start; the page posts are kept. */
function screen(hash: string) {
    const fetch = vi.fn(async (_url: string, init: { body: string }) => {
        const args: string[] = JSON.parse(init.body).args;
        return new Response(
            JSON.stringify({
                exit: 0,
                output: args[0] === "sources" ? sources : { run: "2026-10-12-120000" },
            }),
        );
    });
    vi.stubGlobal("fetch", fetch);
    const status = new CliQuery<Status>([]);
    status.output = example as Status;
    const shown = mount(Downloads, {
        target: document.body,
        props: { hash, status },
    });
    const posted = () => fetch.mock.calls.map(([, init]) => JSON.parse(init.body).args as string[]).filter((args) => args[0] !== "sources");
    return { shown, posted };
}
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

it("lists the downloads by kind without the references, what needs a hand first", async () => {
    const { shown } = screen("#/downloads");
    try {
        await vi.waitFor(() => expect(document.querySelectorAll("details h2")).toHaveLength(4));
        expect([...document.querySelectorAll("details h2")].map((group) => group.textContent)).toEqual([
            "OpenStreetMap",
            "Elevation",
            "Climate and snow",
            "Map styles and tools",
        ]);
        const text = document.body.textContent!;
        expect(text).not.toContain("Prepared Wikimedia");
        expect(text).not.toContain("swissALTI3D");
        const osm = [...document.querySelectorAll("details")[0].querySelectorAll("li")];
        expect(osm[0].textContent, "a stale download comes first, with why").toContain("Due again for this computer: 7 days old; the policy is 7 days");
        expect(osm[0].textContent).toContain("2026-10-05");
        expect(osm[0].textContent).toContain("download.geofabrik.de");
        expect([...osm[0].querySelectorAll(".part")].map((part) => part.textContent)).toEqual(["Bike computer maps", "Planner", "Search", "Trail View"]);
        const tool = [...document.querySelectorAll("li")].find((row) => row.textContent?.includes("tile engine"))!;
        expect(tool.textContent).toContain("Needs a login: GITHUB_TOKEN");
        expect(tool.querySelector<HTMLAnchorElement>('a[href="#/machine"]')).not.toBeNull();
        expect(text).not.toContain("unused");
    } finally {
        await unmount(shown);
    }
});

it("a policy edit opens the review with its flag, and Refresh starts a run", async () => {
    const policy = screen("#/downloads/geofabrik-extracts");
    try {
        await vi.waitFor(() => expect(button("Review This computer with it")).toBeDefined());
        const input = document.querySelector<HTMLInputElement>("input")!;
        const type = (value: string) => {
            input.value = value;
            input.dispatchEvent(new Event("input", { bubbles: true }));
            flushSync();
        };
        type("0");
        expect(button("Review This computer with it").disabled).toBe(true);
        type(" 14 ");
        button("Review This computer with it").click();
        expect(location.hash).toBe(`#/local/review?refresh=${encodeURIComponent("geofabrik-extracts=14")}`);
    } finally {
        await unmount(policy.shown);
    }
    const refresh = screen("#/downloads/natural-earth");
    try {
        await vi.waitFor(() => expect(button("Refresh")).toBeDefined());
        button("Refresh").click();
        await vi.waitFor(() => expect(location.hash).toBe("#/runs/2026-10-12-120000"));
        expect(refresh.posted()).toEqual([["source", "refresh", "natural-earth"]]);
    } finally {
        await unmount(refresh.shown);
    }
});
