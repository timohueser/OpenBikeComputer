// @vitest-environment happy-dom

import { mount, tick, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CatalogClient } from "../../lib/catalog/client";
import { readFileSync } from "node:fs";
import { URL as NodeURL } from "node:url";
const EXAMPLE_ROOT = readFileSync(new NodeURL("../../../../../host/obc-pack/schema/catalog.example.json", import.meta.url), "utf8");
import { DeviceJob } from "../../lib/device/job.svelte";
import CoverageHome from "./CoverageHome.svelte";
import Home from "../../routes/Home.svelte";

vi.mock("./CoverageMap.svelte", async () => ({ default: (await import("../../../test-support/coverage/Selection.svelte")).default }));
vi.mock("./SkinStep.svelte", async () => ({ default: (await import("./ToolIcon.svelte")).default }));
vi.mock("../device/DeviceStep.svelte", async () => ({ default: (await import("./ToolIcon.svelte")).default }));
const host = vi.hoisted(() => ({ catalog: vi.fn(), catalogFetch: vi.fn(async () => new Response("gone", { status: 404 })) }));
vi.mock("../../lib/platform", () => ({ platform: { ...host, name: "web", caps: { deviceDashboard: false }, openMapOutput: null } }));
vi.mock("../../lib/platform/gating", () => ({ available: () => false }));

const client = (body: string, url: string) => CatalogClient.fromBody(body, url, { fetchImpl: host.catalogFetch, attempts: 1 });
async function settle() { for (let n = 0; n < 20; n++) { await Promise.resolve(); await tick(); } }
function button(target: HTMLElement, text: string) {
    return [...target.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent === text)!;
}
afterEach(() => { document.body.replaceChildren(); vi.clearAllMocks(); });

describe("catalog recovery", () => {
    it("keeps selection on refresh failure, then replaces the whole catalog and exposes removed coverage", async () => {
        const target = document.createElement("div");
        document.body.append(target);
        const root = JSON.parse(EXAMPLE_ROOT);
        root.regions = root.regions.filter((r: { id: string }) => r.id !== "europe/switzerland" && !r.id.startsWith("europe/switzerland/"));
        const body = JSON.stringify(root);
        const refreshCatalog = vi.fn()
            .mockRejectedValueOnce(new Error("offline"))
            .mockResolvedValueOnce({ client: client(body, "https://maps.test/new/catalog.json"), body });
        const component = mount(CoverageHome, { target, props: { client: client(EXAMPLE_ROOT, "https://maps.test/old/catalog.json"), rootBody: EXAMPLE_ROOT, refreshCatalog } });
        button(target, "Choose Switzerland").click();
        await settle();
        expect(target.textContent).toContain("Switzerland");
        button(target, "Refresh map catalog").click();
        await settle();
        expect(target.textContent).toContain("could not be refreshed: offline");
        expect(target.querySelector('[data-catalog]')?.getAttribute("data-catalog")).toContain("/old/");
        expect(target.querySelector('[aria-label="Remove Switzerland"]')).not.toBeNull();
        button(target, "Refresh map catalog").click();
        await settle();
        expect(target.querySelector('[data-catalog]')?.getAttribute("data-catalog")).toContain("/new/");
        expect(target.textContent).toContain("This region is no longer in the map catalog");
        expect(document.activeElement).toBe(target.querySelector(".recovery"));
        (target.querySelector('[aria-label="Remove Switzerland"]') as HTMLButtonElement).click();
        await tick();
        expect(target.querySelector('[aria-label="Remove Switzerland"]')).toBeNull();
        expect(refreshCatalog).toHaveBeenCalledTimes(2);
        await unmount(component);
    });

    it("keeps the old catalog if a picked-file device transfer starts during refresh", async () => {
        let finishRefresh!: (result: { client: CatalogClient; body: string }) => void;
        const refreshCatalog = vi.fn(() => new Promise<{ client: CatalogClient; body: string }>((resolve) => { finishRefresh = resolve; }));
        const target = document.createElement("div");
        document.body.append(target);
        const component = mount(CoverageHome, { target, props: { client: client(EXAMPLE_ROOT, "https://maps.test/old/catalog.json"), rootBody: EXAMPLE_ROOT, refreshCatalog } });
        await settle();
        button(target, "Refresh map catalog").click();
        await settle();
        let finishSend!: () => void;
        const job = new DeviceJob("map");
        const send = job.run(() => new Promise<void>((resolve) => { finishSend = resolve; }), () => "sent");
        finishRefresh({ client: client(EXAMPLE_ROOT, "https://maps.test/new/catalog.json"), body: EXAMPLE_ROOT });
        await settle();
        expect(target.querySelector('[data-catalog]')?.getAttribute("data-catalog")).toContain("/old/");
        expect(target.textContent).toContain("Wait for the device transfer");
        expect(button(target, "Refresh map catalog").disabled).toBe(true);
        expect(job.running).toBe(true);
        finishSend();
        await send;
        await settle();
        expect(button(target, "Refresh map catalog").disabled).toBe(false);
        await unmount(component);
    });

    it("offers a native button to refresh an initially unreadable catalog", async () => {
        host.catalog.mockRejectedValueOnce(new Error("removed root"))
            .mockResolvedValueOnce({ url: "https://maps.test/new/catalog.json", body: EXAMPLE_ROOT });
        const target = document.createElement("div");
        document.body.append(target);
        const component = mount(Home, { target });
        await settle();
        expect(target.querySelector('[role="alert"]')?.textContent).toContain("removed root");
        button(target, "Refresh map catalog").click();
        await settle();
        expect(host.catalog).toHaveBeenLastCalledWith({ refresh: true });
        expect(target.querySelector('[data-catalog]')?.getAttribute("data-catalog")).toContain("/new/");
        await unmount(component);
    });
});
