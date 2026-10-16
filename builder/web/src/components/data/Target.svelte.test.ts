// @vitest-environment happy-dom
import { flushSync, mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import areas from "../../../test-support/data/areas.json";
import review from "../../../test-support/data/review.json";
import router from "../../../test-support/data/router.json";
import example from "../../../test-support/data/status.json";
import { CliQuery } from "../../lib/data/query.svelte";
import type { Router, RunList, Status } from "../../lib/data/shapes";
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

/** The server answers the reads with the examples; the commands that change something are kept. */
function server(answer: () => Router = () => router) {
    const fetch = vi.fn(async (_url: string, init: { body: string }) => {
        const args: string[] = JSON.parse(init.body).args;
        const output = { areas, router: answer() }[args[0]] ?? (args.includes("--dry-run") ? review : {});
        return new Response(JSON.stringify({ exit: 0, output }));
    });
    vi.stubGlobal("fetch", fetch);
    // The area picker, the coverage and the releases read `areas` and `router` on their own.
    return () => fetch.mock.calls.map(([, init]) => JSON.parse(init.body).args as string[]).filter((args) => !["areas", "router"].includes(args[0]));
}

function screen(hash: string, stale: boolean, list: RunList = { runs: [] }) {
    const query = new CliQuery<Status>([]);
    query.output = status(stale);
    const runs = new CliQuery<RunList>(["runs"]);
    runs.output = list;
    const name = hash.split("/")[1];
    return { query, screen: mount(Target, { target: document.body, props: { name, hash, status: query, runs } }) };
}

const button = (label: string) => [...document.querySelectorAll("button")].find((button) => button.textContent === label)!;

it("the update screen and two status polls run no command until the person plans", async () => {
    const posted = server();
    const { query, screen: target } = screen("#/local/review", false);
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain("Plan update on"));
        query.output = status(false);
        query.output = status(false);
        flushSync();
        await new Promise((resolve) => setTimeout(resolve, 20));
        expect(posted()).toEqual([]);
    } finally {
        await unmount(target);
    }
});

it("a live run locks the update and the rollback, and only a confirm posts --yes", async () => {
    const posted = server();
    const live = screen("#/prod", false);
    expect(document.body.textContent).toContain("Plan update: locked");
    expect(button("Roll back to 2c91…").disabled).toBe(true);
    expect(document.querySelector('a[href="#/runs/2026-10-12-093112"]')?.textContent).toBe("Watch the run");
    await unmount(live.screen);

    const { screen: target } = screen("#/prod", true);
    try {
        const rollback = example.targets.find(({ target }) => target === "prod")!.release!.rollback!.id;
        button("Roll back to 2c91…").click();
        await vi.waitFor(() => expect(document.body.textContent).toContain("The router still runs 2c91"));
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

it("a link from a source sets the refresh flag of the plan", async () => {
    server();
    const { screen: target } = screen(`#/local/review?refresh=${encodeURIComponent("geofabrik-extracts=14")}`, false);
    const shown = () => [...document.querySelectorAll("code")].map((code) => code.textContent);
    try {
        await vi.waitFor(() => expect(shown()).toContain("obc data update local --refresh geofabrik-extracts=14 --plan"));
    } finally {
        await unmount(target);
    }
});

it("a rollback waits for the router, and the confirm asks the router again", async () => {
    const reason = "not ready: the search service stopped: exit status 1";
    let answer: Router = { ...router, rollback: { ...router.rollback, reason } };
    server(() => answer);
    const { screen: target } = screen("#/prod", true);
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain("Router · serves 7f3a"));
        button("Roll back to 2c91…").click();
        await vi.waitFor(() => expect(document.body.textContent).toContain(`✗ The router does not serve 2c91: ${reason}`));
        expect(button("Roll back now").disabled).toBe(true);
        answer = router;
        button("Cancel").click();
        flushSync();
        button("Roll back to 2c91…").click();
        await vi.waitFor(() => expect(button("Roll back now").disabled).toBe(false));
    } finally {
        await unmount(target);
    }
});

it("Serve starts serve as a run, shows how each app opens the target, and stops that run", async () => {
    const posted = server();
    const idle = screen("#/local", false);
    expect(document.body.textContent).toMatch(/made on laptop, [^·]+ · OSM 2026-10-05/);
    button("Serve local").click();
    await vi.waitFor(() => expect(posted()).toEqual([["serve", "local", "--detach"]]));
    await unmount(idle.screen);

    const run = { id: "2026-10-12-130000", args: ["serve", "local"], started: "2026-10-12T13:00:00Z", state: "running" as const };
    const { screen: target } = screen("#/local", false, { runs: [{ ...run, wall_ms: null, bytes_fetched: 0, bytes_built: 0 }] });
    try {
        expect(document.body.textContent).toContain("iPhone · the Debug app with the launch argument -OBCPlannerCatalog http://192.168.1.20:8710");
        button("Stop serving").click();
        await vi.waitFor(() => expect(posted().at(-1)).toEqual(["runs", run.id, "--stop"]));
    } finally {
        await unmount(target);
    }
});
