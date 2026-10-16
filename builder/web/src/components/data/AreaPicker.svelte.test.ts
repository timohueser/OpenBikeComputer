// @vitest-environment happy-dom
import { flushSync, mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import areas from "../../../test-support/data/areas.json";
import AreaPicker from "./AreaPicker.svelte";

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
});

it("a click on the map ticks the smallest Geofabrik area whose bounds hold the point, also with no search", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ exit: 0, output: areas }))));
    const apply = vi.fn();
    const screen = mount(AreaPicker, { target: document.body, props: { region: "freiburg", boxes: [], apply } });
    const button = (label: string) => [...document.querySelectorAll("button")].find((button) => button.textContent?.trim() === label)!;
    try {
        await vi.waitFor(() => expect(button("Germany")).toBeDefined());
        button("Geofabrik area").click();
        flushSync();
        // The map has no size here, so its centre, 49° N 9° E, is under every point: Baden-Württemberg,
        // Germany and Europe hold it.
        document.querySelector(".leaflet-container")!.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: 0, clientY: 0 }));
        await vi.waitFor(() => expect(button("Use 1 area").disabled).toBe(false));
        button("Use 1 area").click();
        expect(apply).toHaveBeenCalledWith("--region", "europe/germany/baden-wuerttemberg");
    } finally {
        await unmount(screen);
    }
});
