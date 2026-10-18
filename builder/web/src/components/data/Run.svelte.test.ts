// @vitest-environment happy-dom
import { mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import failed from "../../../test-support/data/run-failed.json";
import running from "../../../test-support/data/run-running.json";
import status from "../../../test-support/data/status.json";
import type { RunView, Status } from "../../lib/data/shapes";
import Run from "./Run.svelte";

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
});

/** The server answers each command of `answers`, by its args. */
function serverAnswers(answers: Record<string, unknown>) {
    const fetch = vi.fn(async (_url: string, init: { body: string }) => {
        const args: string[] = JSON.parse(init.body).args;
        return new Response(JSON.stringify({ exit: 0, output: answers[JSON.stringify(args)] }));
    });
    vi.stubGlobal("fetch", fetch);
    return fetch;
}

const text = () => document.body.textContent ?? "";
const button = (label: string) => [...document.querySelectorAll("button")].find((button) => button.textContent === label);

it("shows a run as it goes, and Stop posts exactly the args that it shows", async () => {
    const run = running as RunView;
    // As the CLI does, `--stop` answers with the run as it was: it stops later.
    const fetch = serverAnswers({ [JSON.stringify(["runs", run.id])]: run, [JSON.stringify(["runs", run.id, "--stop"])]: run });
    // Half an hour after device/map started.
    vi.useFakeTimers({ toFake: ["Date"], now: Date.parse("2026-10-12T10:51:10Z") });
    const screen = mount(Run, { target: document.body, props: { id: run.id, status: status as Status } });
    try {
        await vi.waitFor(() => expect(text()).toContain("Layers in this run"));
        expect(text()).toContain("3 of 6 · about 30 min left");
        // Three steps ran at once at most.
        expect(document.querySelector('[aria-labelledby="timeline"] .soft')?.textContent).toBe("7 steps · at most 3 at a time");
        expect([...document.querySelectorAll(".legend li")].map((item) => item.textContent)).toEqual(["device step", "planner step", "fetch", "running"]);
        expect(text()).toContain("laptop · load 6.2 on 10 cores · RAM 12 GB free · store 412 GB free");
        expect(text()).toContain(`The last 4 lines of ${run.log_path}.`);
        expect(document.querySelector("h1")?.textContent).toBe("update local");
        expect(document.querySelector(".phases")?.textContent?.replace(/\s+/g, " ")).toContain("◐ build running");
        expect(text()).toContain("3 of 6");
        expect(text()).toContain("baking 0038-0033 · 0036-0032");
        expect(text()).toContain("10:44:04 device/map 0038-0033 baking");
        expect(document.querySelectorAll("#cells ~ * path, .leaflet-overlay-pane path").length).toBe(5);

        button("Stop…")?.click();
        await vi.waitFor(() => expect(button("Stop the run")).toBeDefined());
        const shown = [...document.querySelectorAll("code")].map((code) => code.textContent);
        expect(shown).toContain(`obc data runs ${run.id} --stop`);
        button("Stop the run")?.click();
        await vi.waitFor(() => expect(text()).toContain("Stopping: the run stops before its next fetch, step or phase."));
        const posted = fetch.mock.calls.map(([, init]) => JSON.parse(init.body).args);
        expect(posted.filter((args) => args.includes("--stop"))).toEqual([["runs", run.id, "--stop"]]);
        expect(document.querySelectorAll("button")).toHaveLength(0);
    } finally {
        vi.useRealTimers();
        await unmount(screen);
    }
});

it("shows why a run failed, its fix, what the store kept, and the review that resumes it", async () => {
    const run = failed as RunView;
    serverAnswers({ [JSON.stringify(["runs", run.id])]: run });
    const screen = mount(Run, { target: document.body, props: { id: run.id, status: status as Status } });
    try {
        const error = await vi.waitFor(() => {
            const shown = document.querySelector("[role=alert]");
            if (!shown) throw new Error("no error shown yet");
            return shown;
        });
        expect(error.textContent).toContain("✗ planner/search failed: the store disk is full");
        expect(error.textContent).toContain("Fix · Free space with obc data clean --apply, or move the store");
        expect(error.textContent).toContain("Kept · device/landmarks, device/map, and planner/search/pois. The next update reuses them.");
        expect(error.querySelector("a")?.getAttribute("href")).toBe(`#/local/resume/${run.id}`);
        expect(error.querySelector(".resume")?.textContent).toBe("obc data update local --add sun --dry-run");
        expect(text()).toContain("local did not change · the local lock is free");
        expect(text()).toContain("stopped at 2 of 3");
        expect(button("Stop…")).toBeUndefined();
    } finally {
        await unmount(screen);
    }
});
