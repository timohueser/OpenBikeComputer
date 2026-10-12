// @vitest-environment happy-dom
import { flushSync, mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import example from "../../../test-support/data/setup.json";
import { CliQuery } from "../../lib/data/query.svelte";
import type { Setup } from "../../lib/data/setup";
import Machine from "./Machine.svelte";

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
});

const report = example as Setup;
const state = (name: string) => {
    const row = [...document.querySelectorAll(".row")].find((row) => row.querySelector(".name")?.textContent === name);
    return row?.querySelector("[role=img]")?.getAttribute("aria-label");
};

it("shows what the machine can do, its tools, credentials and store, and checks again", async () => {
    const setup = new CliQuery<Setup>(["setup"]);
    setup.accept({ ok: true, output: report });
    const screen = mount(Machine, { target: document.body, props: { setup } });
    try {
        flushSync();
        const text = () => document.body.textContent ?? "";
        expect(text()).toContain("This machine: laptop");
        expect(text()).toContain("macOS 15 · 10 cores · 32 GB RAM");
        expect(state("Bake planner layers here")).toBe("missing");
        expect(text()).toContain("Needs GEOS and Java 21.");
        expect(state("Rust")).toBe("ready");
        expect(state("GEOS")).toBe("missing");
        expect(text()).toContain("brew install geos");
        expect(state("R2 key (OBC_R2_*)")).toBe("missing");
        expect(state("OBC_WIKIMEDIA_ENTERPRISE_TOKEN")).toBe("optional");
        expect(text()).toContain("~/.cache/openbikecomputer/store · 0 B used · 412 GB free");
        expect(text()).toContain("Install GEOS and Java 21 (the commands are under Tools)");

        const installed = report.tools.map((tool) => (tool.state === "missing" ? { ...tool, state: "ready", detail: "3.13" } : tool));
        const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({ exit: 0, output: { ...report, tools: installed } })));
        vi.stubGlobal("fetch", fetch);
        document.querySelector("button")?.click();
        await vi.waitFor(() => expect(state("GEOS")).toBe("ready"));
        expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual({ args: ["setup"] });
        expect(text()).toContain("The tools are ready.");
    } finally {
        await unmount(screen);
    }
});
