// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { appearance, startTheme, toggleTheme } from "./theme.svelte";

let stop: (() => void) | undefined;
let system: EventTarget & { matches: boolean };

beforeEach(() => {
    const values = new Map<string, string>();
    vi.stubGlobal("localStorage", {
        getItem: (key: string) => values.get(key) ?? null,
        setItem: (key: string, value: string) => values.set(key, value),
        removeItem: (key: string) => values.delete(key),
    });
    system = Object.assign(new EventTarget(), { matches: false });
    vi.stubGlobal("matchMedia", () => system);
});
afterEach(() => {
    stop?.();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

function systemTheme(dark: boolean) {
    system.matches = dark;
    system.dispatchEvent(new Event("change"));
}

describe("page appearance", () => {
    it("follows the browser until a manual choice, then preserves that choice on reload", () => {
        system.matches = true;
        stop = startTheme();
        expect(document.documentElement.dataset.theme).toBe("dark");
        systemTheme(false);
        expect(appearance.dark).toBe(false);
        toggleTheme();
        expect(localStorage.getItem("obc-theme")).toBe("dark");
        systemTheme(false);
        expect(appearance.dark).toBe(true);
        stop();
        stop = startTheme();
        expect(document.documentElement.dataset.theme).toBe("dark");
    });

    it("shares a landing-page choice and follows changes from another tab", () => {
        localStorage.setItem("obc-theme", "light");
        system.matches = true;
        stop = startTheme();
        expect(appearance.dark).toBe(false);
        localStorage.setItem("obc-theme", "dark");
        window.dispatchEvent(new StorageEvent("storage", { key: "obc-theme" }));
        expect(appearance.dark).toBe(true);
        localStorage.removeItem("obc-theme");
        systemTheme(false);
        window.dispatchEvent(new StorageEvent("storage", { key: "obc-theme" }));
        expect(appearance.dark).toBe(false);
    });

    it("still follows the browser and accepts a toggle when storage is unavailable", () => {
        vi.spyOn(localStorage, "getItem").mockImplementation(() => { throw new Error("blocked"); });
        vi.spyOn(localStorage, "setItem").mockImplementation(() => { throw new Error("blocked"); });
        stop = startTheme();
        systemTheme(true);
        expect(appearance.dark).toBe(true);
        toggleTheme();
        expect(document.documentElement.dataset.theme).toBe("light");
    });
});
