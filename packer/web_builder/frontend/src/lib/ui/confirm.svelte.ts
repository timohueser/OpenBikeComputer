/**
 * "Are you sure?", asked by the app rather than by the browser.
 *
 * ## Why this exists (E3, #913)
 *
 * `window.confirm()` **does not work in the desktop app**, and it does not fail loudly — it returns
 * `false`. WKWebView has no built-in UI for the JavaScript dialogs: it asks its `WKUIDelegate`, and
 * when the delegate does not implement the panel method, no dialog is shown and the call answers
 * `false`. wry *does* install a delegate (unconditionally — `wkwebview/mod.rs` `setUIDelegate`), but
 * `WryWebViewUIDelegate` implements only `runOpenPanel`, the media-capture permission and the
 * new-window request. `runJavaScriptConfirmPanel` (and its alert/prompt siblings) are simply absent,
 * so every `if (!confirm(...)) return;` in this frontend is, inside the app, a statement that reads
 * *"never do this"*.
 *
 * That mattered for four controls, all of them ones only the app has: **Reset to preset**, **Reset
 * a routing profile**, **Remove a category**, and **Clear a cache**. The first is one of the three
 * working-config envelope semantics E3 is answerable for, and it was a no-op on macOS. The others
 * are the style editor and the storage card — the desktop tier's own features. A confirmation that
 * silently answers "no" is worse than none at all: the control looks broken rather than cautious.
 *
 * WebView2 (Chromium) and a browser tab do show the native dialog, so the bug was invisible on two
 * of the three places this frontend runs. Owning the dialog makes all three behave alike, which is
 * the point of one source and three hosts.
 *
 * ## Shape
 *
 * One question at a time, held here rather than in whichever component asked, so the markup is
 * mounted once at the app root and no surface has to carry a modal it only needs occasionally. The
 * promise resolves `false` for every way of declining — the Cancel button, Escape, a click outside
 * — because a caller should never have to tell "no" from "went away".
 */

/** What a confirmation says. Plain data, so the asking site reads as one call. */
export interface ConfirmRequest {
    /** The question, as a sentence. */
    readonly title: string;
    /** What saying yes costs, where that is not obvious from the title. */
    readonly body?: string;
    /** The affirmative button's label. A verb, not "OK" — the button should say what it does. */
    readonly confirmLabel?: string;
    /** Colours the affirmative button as a destructive action. */
    readonly destructive?: boolean;
}

/** A question waiting for an answer, as the dialog component renders it. */
export interface PendingConfirm extends ConfirmRequest {
    readonly answer: (ok: boolean) => void;
}

class ConfirmQueue {
    /** Null when nothing is being asked. Read by `components/ConfirmDialog.svelte`. */
    pending = $state<PendingConfirm | null>(null);

    /**
     * Ask, and resolve with the answer.
     *
     * A second question while one is open resolves `false` immediately rather than queueing. Two
     * modals stacked over each other is never the right screen, and the only way to reach this is a
     * click that got through while a dialog was up — which is exactly the click to decline.
     */
    ask(request: ConfirmRequest): Promise<boolean> {
        if (this.pending) return Promise.resolve(false);
        return new Promise<boolean>((resolve) => {
            this.pending = {
                ...request,
                answer: (ok: boolean) => {
                    this.pending = null;
                    resolve(ok);
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
