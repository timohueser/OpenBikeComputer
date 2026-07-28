/**
 * The app's confirmation, which exists because the browser's answers "no" by itself inside the
 * desktop webview (see the module docs). Two things are worth pinning:
 *
 * - every way of declining resolves `false`, so a caller never has to tell "no" from "went away";
 * - a second question while one is open is declined rather than queued, because the only way to
 *   reach it is a click that got through while a modal was up.
 */

import { describe, expect, it } from "vitest";

import { confirmAction, confirmQueue } from "./confirm.svelte";

describe("confirmAction", () => {
    it("resolves true only when the affirmative button is pressed", async () => {
        const answered = confirmAction({ title: "Re-apply “Bikepacking”?" });
        expect(confirmQueue.pending?.title).toBe("Re-apply “Bikepacking”?");
        confirmQueue.pending!.answer(true);
        expect(await answered).toBe(true);
        expect(confirmQueue.pending, "the dialog closes as soon as it is answered").toBeNull();
    });

    it("resolves false on any way of declining", async () => {
        const declined = confirmAction({ title: "Delete 950 MB?" });
        confirmQueue.pending!.answer(false);
        expect(await declined).toBe(false);
        expect(confirmQueue.pending).toBeNull();
    });

    it("declines a second question rather than stacking two modals", async () => {
        const first = confirmAction({ title: "first" });
        // Not queued: whatever asked this was not something the rider could see or aim at.
        expect(await confirmAction({ title: "second" })).toBe(false);
        expect(confirmQueue.pending?.title, "the open question is untouched").toBe("first");
        confirmQueue.pending!.answer(true);
        expect(await first).toBe(true);
    });

    it("carries the copy the dialog renders", async () => {
        const asked = confirmAction({
            title: "Remove the “water” category?",
            body: "All 12 of its feature types go with it.",
            confirmLabel: "Remove",
            destructive: true,
        });
        expect(confirmQueue.pending).toMatchObject({
            body: "All 12 of its feature types go with it.",
            confirmLabel: "Remove",
            destructive: true,
        });
        confirmQueue.pending!.answer(false);
        await asked;
    });
});
