/**
 * The app's confirmation, which exists because the browser's answers "no" by itself inside the
 * desktop webview (see the module docs). Two things are worth pinning:
 *
 * - every way of declining resolves `false`, so a caller never has to tell "no" from "went away";
 * - a second question while one is open is declined rather than queued, because the only way to
 *   reach it is a click that got through while a modal was up.
 */

import { describe, expect, it } from "vitest";

import { confirmAction, confirmChoice, confirmQueue } from "./confirm.svelte";

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

describe("confirmChoice", () => {
    it("resolves each of the three answers by name", async () => {
        for (const choice of ["confirm", "extra", "cancel"] as const) {
            const asked = confirmChoice({
                title: "Delete the trip “Traverse”?",
                extra: { label: "Delete trip only", destructive: true },
            });
            expect(confirmQueue.pending?.extra?.label).toBe("Delete trip only");
            confirmQueue.pending!.answer(choice);
            expect(await asked).toBe(choice);
            expect(confirmQueue.pending).toBeNull();
        }
    });

    it("keeps the dialog's boolean answers meaning confirm/cancel", async () => {
        const asked = confirmChoice({ title: "still yes/no", extra: { label: "third" } });
        confirmQueue.pending!.answer(true);
        expect(await asked).toBe("confirm");
        const declined = confirmChoice({ title: "still yes/no" });
        confirmQueue.pending!.answer(false);
        expect(await declined).toBe("cancel");
    });

    it("never answers a boolean caller with the extra it could not have asked for", async () => {
        // `confirmAction` maps only "confirm" to true — anything else is a no.
        const asked = confirmAction({ title: "two buttons" });
        confirmQueue.pending!.answer("extra");
        expect(await asked).toBe(false);
    });

    it("declines a second question with cancel rather than stacking", async () => {
        const first = confirmChoice({ title: "first" });
        expect(await confirmChoice({ title: "second", extra: { label: "x" } })).toBe("cancel");
        confirmQueue.pending!.answer("cancel");
        expect(await first).toBe("cancel");
    });
});
