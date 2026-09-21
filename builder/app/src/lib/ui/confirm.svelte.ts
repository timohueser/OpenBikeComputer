/**
 * "Are you sure?", asked by the app rather than by the browser.
 *
 * `window.confirm()` **does not work in the desktop app**, and it does not fail loudly — it returns
 * `false`. WKWebView has no built-in UI for the JavaScript dialogs: it asks its `WKUIDelegate`, and
 * wry's delegate implements `runOpenPanel`, the media-capture permission and the new-window request,
 * but not `runJavaScriptConfirmPanel`. So every `if (!confirm(...)) return;` in this frontend is,
 * inside the app, a statement that reads *"never do this"*.
 *
 * WebView2 and a browser tab do show the native dialog, so the bug was invisible on two of the three
 * places this frontend runs. Owning the dialog makes all three behave alike.
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
