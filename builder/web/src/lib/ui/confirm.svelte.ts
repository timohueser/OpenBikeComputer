/**
 * "Are you sure?", asked by the app rather than by the browser.
 *
 * The app owns the dialog, so the affirmative button names its action and a destructive one looks
 * destructive, which `window.confirm()` cannot do.
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
}

/** A question waiting for an answer, as the dialog component renders it. */
export interface PendingConfirm extends ConfirmRequest {
    readonly answer: (yes: boolean) => void;
}

class ConfirmQueue {
    /** Null when nothing is being asked. Read by `components/ConfirmDialog.svelte`. */
    pending = $state<PendingConfirm | null>(null);

    /**
     * Ask, and resolve with the answer.
     *
     * A second question while one is open resolves `false` immediately rather than queueing.
     * Two modals stacked over each other is never the right screen, and the only way to reach this
     * is a click that got through while a dialog was up — which is exactly the click to decline.
     */
    ask(request: ConfirmRequest): Promise<boolean> {
        if (this.pending) return Promise.resolve(false);
        return new Promise<boolean>((resolve) => {
            this.pending = {
                ...request,
                answer: (yes: boolean) => {
                    this.pending = null;
                    resolve(yes);
                },
            };
        });
    }
}

export const confirmQueue = new ConfirmQueue();

/** The one call a surface makes. `await confirmAction({...})`, in place of `confirm(...)`. */
export function confirmAction(request: ConfirmRequest): Promise<boolean> {
    return confirmQueue.ask(request);
}
