// @vitest-environment happy-dom
import { mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import areas from "../../../test-support/data/areas.json";
import example from "../../../test-support/data/review.json";
import status from "../../../test-support/data/status.json";
import type { Review as ReviewShape, Status } from "../../lib/data/shapes";
import Review from "./Review.svelte";

const review = example as ReviewShape;
const layers = (status as Status).targets.find(({ target }) => target === "local")!.layers!;
const area = (status as Status).targets.find(({ target }) => target === "local")!.area;
const props = { name: "local", kind: "dir" as const, flags: [], machine: "laptop", locked: null, layers, area };

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
    location.hash = "";
});

/** The server answers the review of `local` with `answer`, and a start with a run. */
function server(answer: ReviewShape = review) {
    const fetch = vi.fn(async (_url: string, init: { body: string }) => {
        const args: string[] = JSON.parse(init.body).args;
        if (args[0] === "areas") return new Response(JSON.stringify({ exit: 0, output: areas }));
        const output = args.includes("--dry-run") ? answer : { run: "2026-10-12-120000", review: answer.id };
        return new Response(JSON.stringify({ exit: 0, output }));
    });
    vi.stubGlobal("fetch", fetch);
    // The area picker and the coverage read `areas` on their own.
    return () => fetch.mock.calls.map(([, init]) => JSON.parse(init.body).args as string[]).filter((args) => args[0] !== "areas");
}

const shown = () => [...document.querySelectorAll("code")].map((code) => code.textContent);
const button = (label: string) => [...document.querySelectorAll("button")].find((button) => button.textContent === label)!;

it("starts exactly the shown args with --expect, and only after the format change is accepted", async () => {
    const posted = server();
    const screen = mount(Review, { target: document.body, props });
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain(`Review ${review.id}: update local on laptop`));
        expect(posted()).toEqual([["update", "local", "--dry-run"]]);
        expect(shown()).toContain(`obc data update local --expect ${review.id} --accept-format-change`);
        expect(button("Start on laptop").disabled).toBe(true);

        document.querySelector<HTMLInputElement>(".gate input")!.click();
        await vi.waitFor(() => expect(button("Start on laptop").disabled).toBe(false));
        button("Start on laptop").click();
        await vi.waitFor(() => expect(location.hash).toBe("#/runs/2026-10-12-120000"));
        expect(posted().slice(1)).toEqual([review.args]);
    } finally {
        await unmount(screen);
    }
});

it("a target without a region gets its first region from the area picker", async () => {
    const fetch = vi.fn(async (_url: string, init: { body: string }) => {
        const args: string[] = JSON.parse(init.body).args;
        if (args[0] === "areas") return new Response(JSON.stringify({ exit: 0, output: areas }));
        if (!args.includes("--region")) {
            const error = { code: "usage", message: "target local has no region", fix: "Name one" };
            return new Response(JSON.stringify({ exit: 2, output: { error } }));
        }
        return new Response(JSON.stringify({ exit: 0, output: review }));
    });
    vi.stubGlobal("fetch", fetch);
    const screen = mount(Review, { target: document.body, props: { ...props, layers: [], area: null } });
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain("target local has no region"));
        await vi.waitFor(() => button("Germany").click());
        await vi.waitFor(() => expect(document.body.textContent).toContain(`Review ${review.id}`));
        const reviews = fetch.mock.calls.map(([, init]) => JSON.parse(init.body).args).filter((args) => args[0] !== "areas");
        expect(reviews.at(-1)).toEqual(["update", "local", "--region", "europe/germany", "--dry-run"]);
    } finally {
        await unmount(screen);
    }
});

it("an edit becomes a flag of the next review", async () => {
    const posted = server();
    const screen = mount(Review, { target: document.body, props: { ...props, flags: [["--add", "sun"]] } });
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain(`Review ${review.id}`));
        const snow = [...document.querySelectorAll("label")].find((label) => label.textContent?.trim() === "snow")!;
        snow.querySelector("input")!.click();
        const next = ["update", "local", "--add", "sun", "--add", "snow", "--dry-run"];
        await vi.waitFor(() => expect(posted().at(-1)).toEqual(next));
        expect(shown()).toContain("obc data update local --add sun --add snow --dry-run");
    } finally {
        await unmount(screen);
    }
});

it("on an R2 target, Start says that it publishes to riders and posts the args with --yes", async () => {
    const posted = server();
    const screen = mount(Review, { target: document.body, props: { ...props, name: "prod", kind: "r2" } });
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain(`Publishes review ${review.id} to prod, which serves riders.`));
        document.querySelector<HTMLInputElement>(".gate input")!.click();
        await vi.waitFor(() => expect(button("Start on laptop").disabled).toBe(false));
        button("Start on laptop").click();
        await vi.waitFor(() => expect(posted().slice(1)).toEqual([[...review.args, "--yes"]]));
    } finally {
        await unmount(screen);
    }
});

it("a preset, ticked Geofabrik areas and a box each become the area flag of the next review", async () => {
    const posted = server();
    const screen = mount(Review, { target: document.body, props });
    const type = (input: HTMLInputElement, value: string) => {
        input.value = value;
        input.dispatchEvent(new Event("input", { bubbles: true }));
    };
    const tab = (label: string) => [...document.querySelectorAll<HTMLButtonElement>("[role=tab]")].find((tab) => tab.textContent === label)!.click();
    const last = () => posted().at(-1);
    try {
        await vi.waitFor(() => expect(button("Germany, Austria and Switzerland")).toBeDefined());
        button("Germany, Austria and Switzerland").click();
        await vi.waitFor(() => expect(last()).toEqual(["update", "local", "--region", "dach", "--dry-run"]));

        tab("Geofabrik area");
        await vi.waitFor(() => expect(document.querySelector('input[placeholder="baden-wuerttemberg"]')).not.toBeNull());
        type(document.querySelector('input[placeholder="baden-wuerttemberg"]')!, "baden");
        await vi.waitFor(() => expect(document.querySelectorAll(".found input").length).toBe(2));
        document.querySelectorAll<HTMLInputElement>(".found input").forEach((tick) => tick.click());
        await vi.waitFor(() => expect(button("Review 2 areas").disabled).toBe(false));
        button("Review 2 areas").click();
        const paths = "europe/germany/baden-wuerttemberg,europe/germany/baden-wuerttemberg/freiburg-regbez";
        await vi.waitFor(() => expect(last()).toEqual(["update", "local", "--region", paths, "--dry-run"]));

        tab("Draw a box");
        await vi.waitFor(() => expect(document.querySelectorAll('input[type="number"]').length).toBe(4));
        document.querySelectorAll<HTMLInputElement>('input[type="number"]').forEach((edge, i) => type(edge, ["7.65", "47.85", "8.15", "48.15"][i]));
        button("Review this box").click();
        await vi.waitFor(() => expect(last()).toEqual(["update", "local", "--box", "7.65,47.85,8.15,48.15", "--dry-run"]));
    } finally {
        await unmount(screen);
    }
});

it("a format change names what happens to each reader, and the map draws the cells that bake and go", async () => {
    server();
    const screen = mount(Review, { target: document.body, props });
    const strokes = (color: string) => document.querySelectorAll(`.leaflet-overlay-pane path[stroke="${color}"]`).length;
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain(`Review ${review.id}`));
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

it("shows each note, also two notes of one layer", async () => {
    const note = (country: string) =>
        `terrain-reference/${country} has no version: device/terrain uses GLO-30 there. Fix: obc data source prepare terrain-reference/${country}`;
    server({ ...review, notes: ["ch", "de"].map((country) => ({ layer: "device/terrain", text: note(country) })) });
    const screen = mount(Review, { target: document.body, props });
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain(note("ch")));
        expect(document.body.textContent).toContain(note("de"));
    } finally {
        await unmount(screen);
    }
});
