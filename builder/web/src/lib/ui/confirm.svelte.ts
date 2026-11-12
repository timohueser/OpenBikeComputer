/**
 * "Are you sure?", asked by the app rather than by the browser.
 *
 * The app owns the dialog, so a question can name its action on the button and offer a second
 * affirmative ({@link ConfirmRequest.extra}), which `window.confirm()` cannot.
 *
 * One question at a time, held here rather than in whichever component asked, so the markup is
 * mounted once at the app root. The promise resolves `false` for every way of declining — Cancel,
 * Escape, a click outside — because a caller should never have to tell "no" from "went away".
 */

/** What a confirmation says. Plain data, so the asking site reads as one call. */
export interface ConfirmRequest {
    /** The question, as a sentence. */
    readonly title: string;
    /** What saying yes costs, where that is not obvious from the title. */
    readonly body?: string;
    /** The affirmative button's label. A verb, not "OK": the button should say what it does. */
    readonly confirmLabel?: string;
    /** Colours the affirmative button as a destructive action. */
    readonly destructive?: boolean;
    /**
     * An optional second affirmative, rendered between Cancel and the primary — for the one question
     * with two honest yeses. Only {@link confirmChoice} can see it picked.
     */
    readonly extra?: { readonly label: string; readonly destructive?: boolean };
}

/** The three ways a question with an {@link ConfirmRequest.extra} can resolve. Every way of
 *  declining — Cancel, Escape, a click outside — is `"cancel"`. */
export type ConfirmChoice = "confirm" | "extra" | "cancel";

/** A question waiting for an answer, as the dialog component renders it. Booleans are accepted
 *  as a convenience (`true` ≡ `"confirm"`, `false` ≡ `"cancel"`), so a two-button dialog keeps
 *  reading as yes/no. */
export interface PendingConfirm extends ConfirmRequest {
    readonly answer: (choice: ConfirmChoice | boolean) => void;
}

class ConfirmQueue {
    /** Null when nothing is being asked. Read by `components/ConfirmDialog.svelte`. */
    pending = $state<PendingConfirm | null>(null);

    /**
     * Ask, and resolve with the answer.
     *
     * A second question while one is open resolves `"cancel"` immediately rather than queueing.
     * Two modals stacked over each other is never the right screen, and the only way to reach this
     * is a click that got through while a dialog was up — which is exactly the click to decline.
     */
    ask(request: ConfirmRequest): Promise<ConfirmChoice> {
        if (this.pending) return Promise.resolve("cancel");
        return new Promise<ConfirmChoice>((resolve) => {
            this.pending = {
                ...request,
                answer: (choice: ConfirmChoice | boolean) => {
                    this.pending = null;
                    resolve(typeof choice === "boolean" ? (choice ? "confirm" : "cancel") : choice);
                },
            };
        });
    }
}

export const confirmQueue = new ConfirmQueue();

/** The one call a surface makes. `await confirmAction({...})`, in place of `confirm(...)`. */
export async function confirmAction(request: ConfirmRequest): Promise<boolean> {
    return (await confirmQueue.ask(request)) === "confirm";
}

/** The three-way ask, for the one dialog with a second affirmative ({@link ConfirmRequest.extra}).
 *  Without `extra` it degenerates to `confirmAction` with the answer spelled out. */
export function confirmChoice(request: ConfirmRequest): Promise<ConfirmChoice> {
    return confirmQueue.ask(request);
}
