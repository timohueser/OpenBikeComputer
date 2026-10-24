// @vitest-environment happy-dom
import { flushSync, mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import areasExample from "../../../test-support/data/areas.json";
import planned from "../../../test-support/data/run-planned.json";
import running from "../../../test-support/data/run-running.json";
import sourcesExample from "../../../test-support/data/sources.json";
import status from "../../../test-support/data/status.json";
import type { Status } from "../../lib/data/shapes";
import Bake from "./Bake.svelte";

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
    location.hash = "";
});

const areas = {
    ...areasExample,
    regions: [...areasExample.regions, { id: "central-switzerland", name: "Central Switzerland", countries: ["CH"], kind: "box", box: { west: 7.6, south: 46.46, east: 8.95, north: 47.25 } }],
};
// The Swiss terrain reference covers the area; no machine has the ERA5 login.
const sources = {
    sources: sourcesExample.sources.map((row) =>
        row.id === "era5-land" ? { ...row, credential_missing: true } : row,
    ),
};

/** The server answers the reads, starts a plan run and a bake; the posts that act are kept. */
function screen(hash: string) {
    const fetch = vi.fn(async (_url: string, init: { body: string }) => {
        const args: string[] = JSON.parse(init.body).args;
        const runs: Record<string, unknown> = { [planned.id]: planned, [running.id]: running };
        const output =
            args[0] === "areas"
                ? areas
                : args[0] === "sources"
                  ? sources
                  : args[0] === "runs"
                    ? runs[args[1]]
                    : args.includes("--plan")
                      ? { run: planned.id, review: null }
                      : { run: running.id, review: planned.review.id };
        return new Response(JSON.stringify({ exit: 0, output }));
    });
    vi.stubGlobal("fetch", fetch);
    const shown = mount(Bake, { target: document.body, props: { hash, status: status as Status } });
    const posted = () => fetch.mock.calls.map(([, init]) => JSON.parse(init.body).args as string[]).filter((args) => !["areas", "sources", "runs"].includes(args[0]));
    return { shown, posted };
}
const text = () => document.body.textContent ?? "";
const button = (label: string) => [...document.querySelectorAll("button")].find((button) => button.textContent?.trim().startsWith(label))!;

it("walks area, contents, review and bake, each step from its URL", async () => {
    const area = screen("#/");
    await vi.waitFor(() => expect(document.querySelector<HTMLInputElement>('input[type="search"]')!.disabled).toBe(false));
    const search = document.querySelector<HTMLInputElement>('input[type="search"]')!;
    search.value = "central";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    button("Central Switzerland").click();
    expect(location.hash).toBe("#/bake?region=central-switzerland");
    await unmount(area.shown);

    const contents = screen("#/bake?region=central-switzerland&on=trailview");
    await vi.waitFor(() => expect(text()).toContain("2026-08-20 on R2 · covers central-switzerland"));
    expect(text()).toContain("Needs the login for era5-land");
    expect(document.querySelector<HTMLInputElement>('input[type="checkbox"]:disabled')).not.toBeNull();
    button("Make the plan").click();
    await vi.waitFor(() => expect(location.hash).toBe(`#/bake?region=central-switzerland&on=trailview&plan=${planned.id}`));
    const plan = contents.posted()[0];
    expect(plan.slice(0, 4)).toEqual(["update", "local", "--region", "central-switzerland"]);
    expect(plan).toContain("trailview-ground");
    expect(plan.at(-1)).toBe("--plan");
    await unmount(contents.shown);

    const review = screen(`#/bake?region=central-switzerland&plan=${planned.id}`);
    await vi.waitFor(() => expect(text()).toContain("Peak memory"));
    expect(button("Bake on laptop").disabled, "a format change waits for its checkbox").toBe(true);
    document.querySelector<HTMLInputElement>(".gate input")!.click();
    flushSync();
    button("Bake on laptop").click();
    await vi.waitFor(() => expect(location.hash).toContain(`run=${running.id}`));
    expect(review.posted()).toEqual([planned.review.args]);
    await unmount(review.shown);

    const bake = screen(`#/bake?run=${running.id}`);
    await vi.waitFor(() => expect(text()).toContain("Steps, slowest first"));
    const first = document.querySelector("tbody tr code")!.textContent;
    const slowest = running.steps.filter((step) => step.wall_ms !== null).sort((a, b) => b.wall_ms! - a.wall_ms!)[0];
    expect(first).toBe(slowest.step);
    expect(text()).not.toContain("router ready");
    await unmount(bake.shown);
});
