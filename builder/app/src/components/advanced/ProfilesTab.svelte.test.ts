// @vitest-environment happy-dom

// The profiles editor mounted against the *real* schema `obc-pack schema`
// serves, so the climb-weight cell (#1092) is pinned to the field the packer
// actually reads rather than to a fixture that could drift from it.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { NavProfile, SchemaEnvelope } from "../../lib/config/model";
import { working, type WorkingEnvelope } from "../../lib/config/storage.svelte";
import ProfilesTab from "./ProfilesTab.svelte";

// Resolved from the vitest root (builder/app) — a happy-dom test's
// `import.meta.url` is not a file: URL.
const SCHEMA = JSON.parse(
    readFileSync(resolve(process.cwd(), "../../host/obc-pack/schema/config.schema.json"), "utf8"),
);
const schema = { schema_version: 1, format_version: 12, source: "binary", schema: SCHEMA } as SchemaEnvelope;

function envelope(profiles?: NavProfile[]): WorkingEnvelope {
    return {
        schema_version: 1,
        based_on: null,
        modified: false,
        config: {
            lods: [{ max_mpp: null, simplify: 0 }],
            features: {},
            marker: { color: "0xF800" },
            ...(profiles ? { routing: { profiles } } : {}),
        },
        disabled: [],
    };
}

const mounted: ReturnType<typeof mount>[] = [];

function render(): HTMLElement {
    const target = document.createElement("div");
    document.body.append(target);
    mounted.push(mount(ProfilesTab, { target, props: { schema } }));
    return target;
}

/** Every climb-weight input on screen, one per profile card, in card order. */
function climbInputs(target: HTMLElement): HTMLInputElement[] {
    return [...target.querySelectorAll<HTMLInputElement>('input[aria-label$="climb weight"]')];
}

function type(input: HTMLInputElement, text: string) {
    input.value = text;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new Event("change", { bubbles: true }));
}

beforeEach(() => {
    working.envelope = envelope();
});

afterEach(async () => {
    while (mounted.length) await unmount(mounted.pop()!);
    working.envelope = null;
    document.body.replaceChildren();
});

describe("the climb-weight cell", () => {
    it("shows the shipped weights — Road 10 / Gravel 8 / MTB 6 / Touring 8", () => {
        const target = render();
        const cells = climbInputs(target);
        expect(cells.map((c) => c.getAttribute("aria-label"))).toEqual([
            "Road climb weight",
            "Gravel climb weight",
            "MTB climb weight",
            "Touring climb weight",
        ]);
        expect(cells.map((c) => c.value)).toEqual(["10", "8", "6", "8"]);
        // The bounds come off the schema, not a frontend copy.
        expect(cells[0].min).toBe("0");
        expect(cells[0].max).toBe("255");
    });

    it("writes the edited weight into the config the packer is handed", () => {
        const target = render();
        type(climbInputs(target)[1], "3");
        expect(working.envelope!.config.routing!.profiles[1].climb_weight).toBe(3);
        expect(working.envelope!.modified).toBe(true);
        // 0 is a value, not an unset — a climb-blind profile must be expressible.
        type(climbInputs(target)[1], "0");
        expect(working.envelope!.config.routing!.profiles[1].climb_weight).toBe(0);
    });

    it("refuses an out-of-range weight, reverts the field, and says why", () => {
        const target = render();
        type(climbInputs(target)[0], "900");
        // Nothing reached the model — the config is still `routing`-less, exactly
        // as it was before the rejected keystroke.
        expect(working.envelope!.config.routing).toBeUndefined();
        expect(climbInputs(target)[0].value).toBe("10");
        expect(target.textContent).toContain("climb-blind");
    });

    it("renders a pre-v12 profile as climb-blind rather than blank", () => {
        working.envelope = envelope([{ name: "Legacy", default: 2.0 }]);
        const target = render();
        const cell = climbInputs(target)[0];
        expect(cell.value).toBe("0");
        expect(cell.closest(".cell")?.classList.contains("unstated")).toBe(true);
        // …and it stays absent in the config until someone edits it.
        expect("climb_weight" in working.envelope!.config.routing!.profiles[0]).toBe(false);
    });
});
