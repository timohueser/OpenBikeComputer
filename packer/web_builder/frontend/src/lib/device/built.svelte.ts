/**
 * The map this app built, last, on this machine — remembered between two cards that cannot see
 * each other.
 *
 * #894's whole aim for the desktop tier is *build a map, plug in, one click*, and the two halves of
 * that sentence live in different steps: step 3 owns a `BuildSession` and step 4 owns the device.
 * Neither is the other's parent, and lifting the build session into `Home` would tie a running
 * build's lifetime to a route that also renders the catalog tier. So the one fact they share — a
 * path, a name and a size — lives here, the same shape `deviceHolder` already uses for the session.
 *
 * **It holds a path, not bytes.** That is the point: the file is already on the same disk as the
 * process that owns the USB endpoint, so the send is `usb_send_file` (D4 #909) and nothing is read
 * into the webview. A host whose builds have no path — the FastAPI dev server keeps its output
 * behind a download URL — never populates this, which is why {@link note} takes the path as a
 * required field rather than an optional one.
 */

/** A `.obcm` this app produced, as the device step needs it. */
export interface BuiltMap {
    /** Absolute, inside the app's maps folder — the backend refuses anything else. */
    readonly path: string;
    readonly filename: string;
    readonly bytes: number;
    /** When the build finished, so the UI can say "just now" rather than imply it is current. */
    readonly at: number;
}

class BuiltMapHolder {
    current = $state<BuiltMap | null>(null);

    /**
     * Record a finished build. Idempotent for one result: the build card reports from an effect,
     * which re-runs whenever anything else on the card changes, and re-noting the same file would
     * clear a "sent" badge the rider is still reading.
     */
    note(map: Omit<BuiltMap, "at">): void {
        if (this.current?.path === map.path) return;
        this.current = { ...map, at: Date.now() };
    }

    /** Forget it — what a new build starting calls, so the previous map cannot be sent by a click
     *  aimed at the one now running. */
    clear(): void {
        this.current = null;
    }
}

export const builtMap = new BuiltMapHolder();
