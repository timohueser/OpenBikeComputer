// @vitest-environment happy-dom
import { mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import plan from "../../../test-support/data/clean.json";
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
        expect(text()).toContain("Needs GEOS and Java 21+.");
        expect(state("Rust")).toBe("ready");
        expect(text()).toContain("brew install openjdk@21");
        expect(state("R2 key (OBC_R2_*)")).toBe("missing");
        expect(state("OBC_REFERENCE_SE_USER and OBC_REFERENCE_SE_PASSWORD")).toBe("optional");
        expect(text()).toContain("/Users/rider/.cache/openbikecomputer/store · 0 B used · 412 GB free");
        expect(text()).toContain("Install GEOS and Java 21+ (the commands are under Tools)");

        document.querySelector("button")?.click();
        await vi.waitFor(() => expect(state("GEOS")).toBe("ready"));
        expect(fetch.mock.calls.map(([, init]) => JSON.parse(init.body))).toEqual([{ args: ["setup"] }, { args: ["setup"] }]);
        expect(text()).toContain("Tools are ready. Ask the owner for an R2 key only if you will publish to prod.");
    } finally {
        await unmount(screen);
    }
});

it("Clean shows the plan, and its confirm posts exactly `clean --apply --yes`", async () => {
    const fetch = vi.fn(async (_url: string, init: { body: string }) => {
        const args: string[] = JSON.parse(init.body).args;
        return new Response(JSON.stringify({ exit: 0, output: args[0] === "setup" ? report : plan }));
    });
    vi.stubGlobal("fetch", fetch);
    const screen = mount(Machine, { target: document.body, props: { machine: status.machine } });
    const button = (label: string) => [...document.querySelectorAll("button")].find((button) => button.textContent === label)!;
    try {
        await vi.waitFor(() => expect(button("Clean…")).toBeDefined());
        button("Clean…").click();
        await vi.waitFor(() => expect(button("Clean now")).toBeDefined());
        const text = document.body.textContent?.replace(/\s+/g, " ");
        expect(text).toContain("A clean removes 437 MB: 1 record, 1 object and 50 MB of unfinished work.");
        expect(text).toContain("It keeps 3.4 GB: served by local, newest of a request, served by prod, and inputs kept.");
        button("Clean now").click();
        await vi.waitFor(() => expect(document.body.textContent).toContain("Removed 437 MB from the store."));
        const posted = fetch.mock.calls.map(([, init]) => JSON.parse(init.body).args);
        expect(posted.filter((args) => args[0] === "clean")).toEqual([["clean"], ["clean", "--apply", "--yes"]]);
    } finally {
        await unmount(screen);
    }
});

it("a missing credential posts its value to the credential route, and the page never shows it", async () => {
    const value = "fake-token-not-a-real-credential";
    const set = { file: "/Users/rider/.config/openbikecomputer/credentials.env", names: ["OBC_REFERENCE_DK_TOKEN"], sources: ["dtm-dk"], overridden: [] };
    const fetch = vi.fn(async (url: string, _init: { body: string }) => {
        return new Response(JSON.stringify({ exit: 0, output: url === "/api/credential" ? set : report }));
    });
    vi.stubGlobal("fetch", fetch);
    const screen = mount(Machine, { target: document.body, props: { machine: status.machine } });
    const input = () => document.querySelector<HTMLInputElement>("input[name=OBC_REFERENCE_DK_TOKEN]");
    try {
        await vi.waitFor(() => expect(input()).not.toBeNull());
        const row = input()!.closest(".row")!;
        expect(row.textContent).toContain("For Terrain in Denmark");
        expect(row.querySelector("a")?.getAttribute("href")).toBe("https://dataforsyningen.dk");
        expect(input()!.type).toBe("password");
        input()!.value = value;
        row.querySelector<HTMLButtonElement>("button[type=submit]")!.click();
        await vi.waitFor(() => expect(row.textContent).toContain("Saved OBC_REFERENCE_DK_TOKEN in"));
        const posted = fetch.mock.calls.filter(([url]) => url === "/api/credential").map(([, init]) => JSON.parse(init.body));
        expect(posted).toEqual([{ source: "dtm-dk", values: [value] }]);
        await vi.waitFor(() => expect(fetch.mock.calls.filter(([url]) => url === "/api/cli")).toHaveLength(2));
        expect(document.body.innerHTML).not.toContain(value);
        expect([...document.querySelectorAll("input")].map((input) => input.value)).not.toContain(value);
        expect(state("~/.cdsapirc")).toBe("ready");
        expect([...document.querySelectorAll("button")].filter((button) => button.textContent === "Replace")).toHaveLength(1);
    } finally {
        await unmount(screen);
    }
});
