// @vitest-environment happy-dom
import { flushSync, mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import areasExample from "../../../test-support/data/areas.json";
import review from "../../../test-support/data/prepare-review.json";
import sources from "../../../test-support/data/sources.json";
import example from "../../../test-support/data/status.json";
import { CliQuery } from "../../lib/data/query.svelte";
import { screenAt } from "../../lib/data/screens";
import type { Status } from "../../lib/data/shapes";
import References from "./References.svelte";

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
    location.hash = "";
});

const areas = {
    ...areasExample,
    regions: [
        ...areasExample.regions,
        {
            id: "central-switzerland",
            name: "Central Switzerland",
            countries: ["CH"],
            kind: "box",
            box: { west: 7.6, south: 46.46, east: 8.95, north: 47.25 },
        },
    ],
};

/** The server answers the reads, a prepare review and a start; the page posts are kept. */
function screen(hash: string, loaded = true) {
    const fetch = vi.fn(async (_url: string, init: { body: string }) => {
        const args: string[] = JSON.parse(init.body).args;
        const output =
            args[0] === "sources"
                ? sources
                : args[0] === "areas"
                  ? areas
                  : args.includes("--dry-run")
                    ? review
                    : { run: "2026-10-12-120000", review: review.id };
        return new Response(JSON.stringify({ exit: 0, output }));
    });
    vi.stubGlobal("fetch", fetch);
    const status = new CliQuery<Status>([]);
    if (loaded) status.output = example as Status;
    const shown = mount(References, {
        target: document.body,
        props: { hash, status },
    });
    const posted = () => fetch.mock.calls.map(([, init]) => JSON.parse(init.body).args as string[]).filter((args) => !["sources", "areas"].includes(args[0]));
    return { shown, posted, status };
}
const text = () => document.body.textContent ?? "";
const button = (label: string) => [...document.querySelectorAll("button")].find((button) => button.textContent?.trim().startsWith(label))!;
const section = (title: string) => [...document.querySelectorAll("section")].find((at) => at.querySelector("h2")?.textContent === title)!;

it("shows each family with the state of each member and the action that makes the next version", async () => {
    const { shown } = screen("#/references");
    try {
        await vi.waitFor(() => expect(text()).toContain("Terrain from the national LiDAR models"));
        expect([...document.querySelectorAll("section h2")].map((h2) => h2.textContent)).toEqual(["Wiki", "Terrain", "Forest"]);
        const terrain = section("Terrain");
        expect([...terrain.querySelectorAll(".legend li")].map((li) => li.textContent)).toEqual(["On R2 · 1"]);
        const [row] = [...terrain.querySelectorAll(".members > li")];
        expect(row.textContent).toContain("2026-08-20 · 5.0 GB · On R2 · Central Switzerland");
        expect(row.textContent).toContain("This computer cannot be read");
        const add = row.querySelector<HTMLAnchorElement>("a.act")!;
        expect([add.textContent, add.getAttribute("href")]).toEqual(["Add an area", "#/references/terrain/prepare?to=prod"]);
        const forest = section("Forest");
        expect([...forest.querySelectorAll(".legend li")].map((li) => li.textContent)).toEqual([
            "Lock left by a stopped run · 1",
            "Being prepared · 1",
            "On this computer only · 1",
        ]);
        const rows = [...forest.querySelectorAll(".members > li")];
        expect(rows[0].textContent).toContain("A run on laptop stopped without freeing the lock");
        const actions = rows.map((row) => row.querySelector<HTMLAnchorElement>("a.act"));
        expect([actions[0]!.textContent, actions[0]!.getAttribute("href")]).toEqual(["Prepare", "#/references/forest-reference%2Fat/prepare?to=prod"]);
        expect(actions[1], "a member that a run prepares has no action").toBeNull();
        expect(forest.textContent, "a version of this computer names its area").toContain("On this computer · Central Switzerland");
    } finally {
        await unmount(shown);
    }
});

it("the prepare link of the Bake screen opens the review of its area, and terrain adds an area on R2", async () => {
    const hash = "#/sources/forest-reference%2Fch/prepare?to=local&region=central-switzerland";
    expect(screenAt(hash).id).toBe("references");
    expect(screenAt("#/sources/natural-earth").id).toBe("downloads");
    const local = screen(hash);
    try {
        await vi.waitFor(() => expect(text()).toContain("Covers Central Switzerland"));
        expect(local.posted(), "a review waits for its button").toEqual([]);
        button("Review preparation").click();
        const args = ["source", "prepare", "forest-reference/ch", "--to", "local", "--region", "central-switzerland", "--dry-run"];
        await vi.waitFor(() => expect(local.posted()).toEqual([args]));
        button("On R2").click();
        expect(location.hash).toBe("#/references/forest-reference%2Fch/prepare?to=prod&region=central-switzerland");
    } finally {
        await unmount(local.shown);
    }
    document.body.replaceChildren();
    const forest = screen("#/references/forest-reference%2Fch/prepare?to=prod&region=central-switzerland");
    try {
        await vi.waitFor(() => expect(forest.posted()).toEqual([["source", "prepare", "forest-reference/ch", "--to", "prod", "--dry-run"]]));
        expect(button("Review preparation"), "R2 holds all of a forest member").toBeUndefined();
    } finally {
        await unmount(forest.shown);
    }
    document.body.replaceChildren();
    const terrain = screen("#/references/terrain/prepare?to=prod&region=central-switzerland");
    try {
        await vi.waitFor(() => expect(text()).toContain("Covers Central Switzerland"));
        expect(text(), "a version of terrain adds an area, never all of the world").not.toContain("All of it");
        button("Review preparation").click();
        const args = ["source", "prepare", "terrain", "--to", "prod", "--region", "central-switzerland", "--dry-run"];
        await vi.waitFor(() => expect(terrain.posted()).toEqual([args]));
    } finally {
        await unmount(terrain.shown);
    }
});

it("a wiki review pins the region and languages, and changing them hides Start", async () => {
    const { shown, posted, status } = screen("#/references/wiki/prepare?to=prod&region=europe/dach", false);
    try {
        await vi.waitFor(() => expect(button("Review preparation")).toBeDefined());
        expect(posted()).toEqual([]);
        for (const code of ["fr", "es"]) document.querySelector<HTMLInputElement>(`input[value="${code}"]`)!.click();
        flushSync();
        button("Review preparation").click();
        await vi.waitFor(() => expect(text()).toContain(`Version ${review.version}`));
        expect(posted()).toEqual([["source", "prepare", "wiki", "--to", "prod", "--region", "europe/dach", "--languages", "en,de", "--dry-run"]]);
        expect(text()).toContain("Languages · English and German");
        expect(text()).toContain("Files · counted by the run · text published");
        expect(button("Start").disabled, "the kind of the target is not known yet").toBe(true);
        status.output = example as Status;
        flushSync();
        const german = document.querySelector<HTMLInputElement>('input[value="de"]')!;
        german.click();
        flushSync();
        expect(button("Start")).toBeUndefined();
        german.click();
        flushSync();
        button("Start").click();
        await vi.waitFor(() => expect(location.hash).toBe("#/runs/2026-10-12-120000"));
        expect(posted().at(-1)).toEqual([...review.args, "--yes"]);
    } finally {
        await unmount(shown);
    }
});
