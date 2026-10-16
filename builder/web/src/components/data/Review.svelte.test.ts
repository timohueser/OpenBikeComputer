// @vitest-environment happy-dom
import { mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import areas from "../../../test-support/data/areas.json";
import example from "../../../test-support/data/review.json";
import plannedExample from "../../../test-support/data/run-planned.json";
import status from "../../../test-support/data/status.json";
import type { Review as ReviewShape, RunView, Status } from "../../lib/data/shapes";
import Review from "./Review.svelte";

const review = example as unknown as ReviewShape;
const planned = plannedExample as unknown as RunView;
const layers = (status as Status).targets.find(({ target }) => target === "local")!.layers!;
const area = (status as Status).targets.find(({ target }) => target === "local")!.area;
const props = { name: "local", kind: "dir" as const, flags: [], machine: "laptop", locked: null, layers, area };

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
    location.hash = "";
});

/** The server starts a plan run, answers its view with `run`, and starts a bake. */
function server(run: RunView = planned) {
    const fetch = vi.fn(async (_url: string, init: { body: string }) => {
        const args: string[] = JSON.parse(init.body).args;
        if (args[0] === "areas") return new Response(JSON.stringify({ exit: 0, output: areas }));
        if (args[0] === "runs") return new Response(JSON.stringify({ exit: 0, output: run }));
        const output = args.includes("--plan") ? { run: run.id, review: null } : { run: "2026-10-12-120000", review: run.review?.id };
        return new Response(JSON.stringify({ exit: 0, output }));
    });
    vi.stubGlobal("fetch", fetch);
    // The area picker reads `areas`, and the screen polls the plan run.
    return () => fetch.mock.calls.map(([, init]) => JSON.parse(init.body).args as string[]).filter((args) => !["areas", "runs"].includes(args[0]));
}

const shown = () => [...document.querySelectorAll("code")].map((code) => code.textContent);
const button = (label: string) => [...document.querySelectorAll("button")].find((button) => button.textContent === label)!;

it("plans with the shown settings, and starts exactly the args of the plan after the format change is accepted", async () => {
    const posted = server();
    const screen = mount(Review, { target: document.body, props });
    try {
        await vi.waitFor(() => expect(button("Plan update on laptop").disabled).toBe(false));
        expect(shown()).toContain("obc data update local --plan");
        button("Plan update on laptop").click();
        await vi.waitFor(() => expect(document.body.textContent).toContain(`Plan ${review.id}: update local on laptop`));
        expect(posted()).toEqual([["update", "local", "--plan"]]);
        expect(location.hash).toBe(`#/local/review?plan=${planned.id}`);
        expect(button("Start bake on laptop").disabled).toBe(true);

        document.querySelector<HTMLInputElement>(".gate input")!.click();
        await vi.waitFor(() => expect(button("Start bake on laptop").disabled).toBe(false));
        button("Start bake on laptop").click();
        await vi.waitFor(() => expect(location.hash).toBe("#/runs/2026-10-12-120000"));
        expect(posted().slice(1)).toEqual([review.args]);
    } finally {
        await unmount(screen);
    }
});

it("a running plan shows its fetches and its log, and an edit makes the plan outdated", async () => {
    const fetches = [{ source: "geofabrik-extracts", version: "2026-10-15", params: [["area", "europe/dach"]], bytes: null, error: null }];
    const running = { ...planned, state: "running", wall_ms: null, review: null, fetches } as unknown as RunView;
    server(running);
    const screen = mount(Review, { target: document.body, props: { ...props, plan: planned.id } });
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain("geofabrik-extracts (europe/dach) · fetching…"));
        expect(document.body.textContent).toContain("1.2 GiB / 6.2 GiB downloaded");
        expect(button("Plan again").disabled).toBe(true);
        await unmount(screen);

        server();
        const done = mount(Review, { target: document.body, props: { ...props, plan: planned.id } });
        try {
            await vi.waitFor(() => expect(document.body.textContent).toContain(`Plan ${review.id}`));
            button("Freiburg").click();
            await vi.waitFor(() => expect(document.body.textContent).toContain("The settings changed since this plan."));
            expect(button("Start bake on laptop").disabled).toBe(true);
            expect(shown()).toContain("obc data update local --region freiburg --plan");
        } finally {
            await unmount(done);
        }
    } finally {
        document.body.replaceChildren();
    }
});

it("a plan whose run gives the settings back in another order is the plan of these settings", async () => {
    // The page edits in its own order; `obc data update` gives the settings back in its order.
    const args = ["update", "local", "--region", "dach", "--refresh", "wiki=30", "--plan"];
    const posted = server({ ...planned, args });
    const screen = mount(Review, { target: document.body, props: { ...props, flags: [["--refresh", "wiki=30"]] } });
    try {
        await vi.waitFor(() => expect(button("Germany, Austria and Switzerland")).toBeDefined());
        button("Germany, Austria and Switzerland").click();
        await vi.waitFor(() => expect(shown()).toContain("obc data update local --refresh wiki=30 --region dach --plan"));
        button("Plan update on laptop").click();
        await vi.waitFor(() => expect(document.body.textContent).toContain(`Plan ${review.id}`));
        expect(posted()).toEqual([["update", "local", "--refresh", "wiki=30", "--region", "dach", "--plan"]]);
        expect(document.body.textContent).not.toContain("The settings changed since this plan.");
        document.querySelector<HTMLInputElement>(".gate input")!.click();
        await vi.waitFor(() => expect(button("Start bake on laptop").disabled).toBe(false));
    } finally {
        await unmount(screen);
    }
});

it("a target without a region plans only after the area picker names one", async () => {
    const posted = server();
    const screen = mount(Review, { target: document.body, props: { ...props, layers: [], area: null } });
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain("Choose an area first."));
        expect(button("Plan update on laptop").disabled).toBe(true);
        await vi.waitFor(() => button("Germany").click());
        await vi.waitFor(() => expect(button("Plan update on laptop").disabled).toBe(false));
        button("Plan update on laptop").click();
        await vi.waitFor(() => expect(posted()).toEqual([["update", "local", "--region", "europe/germany", "--plan"]]));
    } finally {
        await unmount(screen);
    }
});

it("on an R2 target, Start says that it publishes to riders and posts the args with --yes", async () => {
    const posted = server();
    const screen = mount(Review, { target: document.body, props: { ...props, name: "prod", kind: "r2", plan: planned.id } });
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain(`Publishes plan ${review.id} to prod, which serves riders.`));
        document.querySelector<HTMLInputElement>(".gate input")!.click();
        await vi.waitFor(() => expect(button("Start bake on laptop").disabled).toBe(false));
        button("Start bake on laptop").click();
        await vi.waitFor(() => expect(posted()).toEqual([[...review.args, "--yes"]]));
    } finally {
        await unmount(screen);
    }
});

it("a preset, ticked Geofabrik areas and a box each become the area flag of the next plan", async () => {
    server();
    const screen = mount(Review, { target: document.body, props });
    const type = (input: HTMLInputElement, value: string) => {
        input.value = value;
        input.dispatchEvent(new Event("input", { bubbles: true }));
    };
    const tab = (label: string) => [...document.querySelectorAll<HTMLButtonElement>("[role=tab]")].find((tab) => tab.textContent === label)!.click();
    const plans = (...flags: string[]) => vi.waitFor(() => expect(shown()).toContain(`obc data update local ${flags.join(" ")} --plan`));
    try {
        await vi.waitFor(() => expect(button("Germany, Austria and Switzerland")).toBeDefined());
        button("Germany, Austria and Switzerland").click();
        await plans("--region", "dach");

        tab("Geofabrik area");
        await vi.waitFor(() => expect(document.querySelector('input[placeholder="baden-wuerttemberg"]')).not.toBeNull());
        type(document.querySelector('input[placeholder="baden-wuerttemberg"]')!, "baden");
        await vi.waitFor(() => expect(document.querySelectorAll(".found input").length).toBe(2));
        document.querySelectorAll<HTMLInputElement>(".found input").forEach((tick) => tick.click());
        // A ticked area stays listed when the search changes.
        type(document.querySelector('input[placeholder="baden-wuerttemberg"]')!, "nothing matches");
        await vi.waitFor(() => expect(document.querySelectorAll(".found input:checked").length).toBe(2));
        button("Use 2 areas").click();
        await plans("--region", "europe/germany/baden-wuerttemberg,europe/germany/baden-wuerttemberg/freiburg-regbez");

        tab("Draw a box");
        await vi.waitFor(() => expect(document.querySelectorAll('input[type="number"]').length).toBe(4));
        document.querySelectorAll<HTMLInputElement>('input[type="number"]').forEach((edge, i) => type(edge, ["7.65", "47.85", "8.15", "48.15"][i]));
        button("Use this box").click();
        await plans("--box", "7.65,47.85,8.15,48.15");
    } finally {
        await unmount(screen);
    }
});

it("a format change names what happens to each reader, and the map draws the cells that bake and go", async () => {
    server();
    const screen = mount(Review, { target: document.body, props: { ...props, plan: planned.id } });
    const strokes = (color: string) => document.querySelectorAll(`.leaflet-overlay-pane path[stroke="${color}"]`).length;
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain(`Plan ${review.id}`));
        const rows = [...document.querySelectorAll(".format tbody tr")].map((row) => [...row.querySelectorAll("td")].map((cell) => cell.textContent));
        expect(rows.map((row) => row.slice(0, 4))).toEqual([
            ["device/map", "device", "19", "20"],
            ["device/map", "map builder", "19", "20"],
        ]);
        expect(rows[0][4]).toContain("Maps already on a device keep working");
        expect(document.body.textContent).toContain("■ 2 cells bake · ■ 1 cell go");
        await vi.waitFor(() => expect([strokes("var(--running)"), strokes("var(--missing)")]).toEqual([2, 1]));
        expect(document.body.textContent).toContain("device/terrain · terrain-reference/de has no version");
    } finally {
        await unmount(screen);
    }
});

it("counts the params of a fetch, and says which costs are unknown", async () => {
    server();
    const some = mount(Review, { target: document.body, props: { ...props, plan: planned.id } });
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain("1 of 3 bakes have no estimate"));
        expect(document.body.textContent).toContain("at least");
        expect(document.body.textContent).toContain("osm 2026-10-11 (europe/germany/baden-wuerttemberg, ");
    } finally {
        await unmount(some);
    }
    const tiles = (...names: string[]) => names.map((name) => ["tile", name] as [string, string]);
    const fetches = [
        { source: "copernicus-glo-30", version: "2022-05-09", params: tiles("N47_E007", "N47_E008"), files: [], bytes: null },
        { source: "copernicus-glo-30", version: "2022-05-09", params: tiles("N48_E008"), files: [], bytes: null },
    ];
    const cost = { ...review.cost, unknown: { ...review.cost.unknown, estimate: review.bakes.map(({ step }) => step) } };
    server({ ...planned, review: { ...review, fetches, cost } });
    const none = mount(Review, { target: document.body, props: { ...props, plan: planned.id } });
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain("copernicus-glo-30 2022-05-09 (3 tiles)"));
        expect(document.body.textContent).toContain("time, memory and store size unknown: none of the 3 bakes ran on this machine before");
    } finally {
        await unmount(none);
    }
});

it("shows each note, also two notes of one layer", async () => {
    const note = (country: string) =>
        `terrain-reference/${country} has no version: device/terrain uses GLO-30 there. Fix: obc data source prepare terrain-reference/${country}`;
    const notes = ["ch", "de"].map((country) => ({ layer: "device/terrain", text: note(country) }));
    server({ ...planned, review: { ...review, notes } });
    const screen = mount(Review, { target: document.body, props: { ...props, plan: planned.id } });
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain(note("ch")));
        expect(document.body.textContent).toContain(note("de"));
    } finally {
        await unmount(screen);
    }
});
