// @vitest-environment happy-dom
import { mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import example from "../../../test-support/data/setup.json";
import status from "../../../test-support/data/status.json";
import type { Setup } from "../../lib/data/shapes";
import Machine from "./Machine.svelte";

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
});

const report = example as Setup;
const answer = (output: Setup) => new Response(JSON.stringify({ exit: 0, output }));
const state = (name: string) => {
    const row = [...document.querySelectorAll(".row")].find((row) => row.querySelector(".name")?.textContent === name);
    return row?.querySelector("[role=img]")?.getAttribute("aria-label");
};

it("runs `obc data setup` when it opens, shows what the machine can do, and checks again", async () => {
    const installed = report.tools.map((tool) => (tool.state === "missing" ? { ...tool, state: "ready" as const, detail: "3.13" } : tool));
    const fetch = vi.fn().mockResolvedValueOnce(answer(report)).mockResolvedValueOnce(answer({ ...report, tools: installed }));
    vi.stubGlobal("fetch", fetch);
    const screen = mount(Machine, { target: document.body, props: { machine: status.machine } });
    try {
        await vi.waitFor(() => expect(state("GEOS")).toBe("missing"));
        const text = () => document.body.textContent ?? "";
        expect(text()).toContain("This machine: laptop");
        expect(text()).toContain("macOS · 10 cores · 32 GB RAM · checked at");
        expect(state("Bake planner layers here")).toBe("missing");
        expect(text()).toContain("Needs GEOS and Java.");
        expect(state("Rust")).toBe("ready");
        expect(text()).toContain("brew install openjdk@21");
        expect(state("R2 key (OBC_R2_*)")).toBe("missing");
        expect(state("OBC_REFERENCE_SE_USER and OBC_REFERENCE_SE_PASSWORD")).toBe("optional");
        expect(text()).toContain("/Users/rider/.cache/openbikecomputer/store · 0 B used · 412 GB free");
        expect(text()).toContain("Install GEOS and Java (the commands are under Tools)");

        document.querySelector("button")?.click();
        await vi.waitFor(() => expect(state("GEOS")).toBe("ready"));
        expect(fetch.mock.calls.map(([, init]) => JSON.parse(init.body))).toEqual([{ args: ["setup"] }, { args: ["setup"] }]);
        expect(text()).toContain("Tools are ready. Ask the owner for an R2 key only if you will publish to prod.");
    } finally {
        await unmount(screen);
    }
});
