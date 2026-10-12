// @vitest-environment happy-dom
import { flushSync, mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import areas from "../../../test-support/data/areas.json";
import review from "../../../test-support/data/review.json";
import example from "../../../test-support/data/status.json";
import { CliQuery } from "../../lib/data/query.svelte";
import type { Status } from "../../lib/data/shapes";
import Target from "./Target.svelte";

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
});

/** A status from the example; `stale` says whether the heartbeat of the prod lock is old. */
function status(stale: boolean): Status {
    const status = structuredClone(example) as Status;
    status.targets.find(({ target }) => target === "prod")!.lock!.stale = stale;
    return status;
}

function server() {
    const fetch = vi.fn(async (_url: string, init: { body: string }) => {
        const args: string[] = JSON.parse(init.body).args;
        if (args[0] === "areas") return new Response(JSON.stringify({ exit: 0, output: areas }));
        return new Response(JSON.stringify({ exit: 0, output: args.includes("--dry-run") ? review : {} }));
    });
    vi.stubGlobal("fetch", fetch);
    // The area picker and the coverage read `areas` on their own.
    return () => fetch.mock.calls.map(([, init]) => JSON.parse(init.body).args as string[]).filter((args) => args[0] !== "areas");
}

function screen(hash: string, stale: boolean) {
    const query = new CliQuery<Status>([]);
    query.output = status(stale);
    const name = hash.split("/")[1];
    return { query, screen: mount(Target, { target: document.body, props: { name, hash, status: query } }) };
}

const button = (label: string) => [...document.querySelectorAll("button")].find((button) => button.textContent === label)!;

it("two status polls with the same target start one review", async () => {
    const posted = server();
    const { query, screen: target } = screen("#/local/review", false);
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain(`Review ${review.id}`));
        query.output = status(false);
        query.output = status(false);
        flushSync();
        await new Promise((resolve) => setTimeout(resolve, 20));
        expect(posted()).toEqual([["update", "local", "--dry-run"]]);
    } finally {
        await unmount(target);
    }
});

it("a live run locks the update and the rollback, and only a confirm posts --yes", async () => {
    const posted = server();
    const live = screen("#/prod", false);
    expect(document.body.textContent).toContain("Review update: locked");
    expect(button("Roll back to 2c91…").disabled).toBe(true);
    expect(document.querySelector('a[href="#/runs/2026-10-12-093112"]')?.textContent).toBe("Watch the run");
    await unmount(live.screen);

    const { screen: target } = screen("#/prod", true);
    try {
        const rollback = example.targets.find(({ target }) => target === "prod")!.release!.rollback!.id;
        button("Roll back to 2c91…").click();
        flushSync();
        expect(document.body.textContent).toContain("The router still runs 2c91");
        button("Take over the lock…").click();
        flushSync();
        expect(posted()).toEqual([]);
        button("Take over the lock").click();
        await vi.waitFor(() => expect(posted()).toEqual([["unlock", "prod", "--from", "hetzner-bake-1", "--yes"]]));
        button("Roll back to 2c91…").click();
        flushSync();
        button("Roll back now").click();
        await vi.waitFor(() => expect(posted().at(-1)).toEqual(["rollback", "prod", "--expect", rollback, "--yes"]));
    } finally {
        await unmount(target);
    }
});

it("the coverage colours the leaves of the chosen tiled layer by their state", async () => {
    server();
    const { screen: target } = screen("#/prod", false);
    const strokes = (color: string) => document.querySelectorAll(`.leaflet-overlay-pane path[stroke="${color}"]`).length;
    try {
        await vi.waitFor(() => expect(strokes("var(--ready)")).toBe(6));
        const select = document.querySelector<HTMLSelectElement>('[aria-labelledby="coverage"] select')!;
        select.value = "device/map";
        select.dispatchEvent(new Event("change", { bubbles: true }));
        await vi.waitFor(() => expect([strokes("var(--stale)"), strokes("var(--ready)")]).toEqual([4, 2]));
        expect(document.body.textContent).toContain("● stale 4");
    } finally {
        await unmount(target);
    }
});

it("a link from a source sets the refresh flag of the review", async () => {
    const posted = server();
    const { screen: target } = screen(`#/local/review?refresh=${encodeURIComponent("geofabrik-extracts=14")}`, false);
    try {
        await vi.waitFor(() => expect(posted()).toEqual([["update", "local", "--refresh", "geofabrik-extracts=14", "--dry-run"]]));
    } finally {
        await unmount(target);
    }
});
