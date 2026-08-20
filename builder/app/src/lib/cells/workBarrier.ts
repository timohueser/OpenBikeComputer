// OPFS map work is origin-global, while DownloadStep is only component-local.
// A remount (or a second builder surface) must therefore wait for the previous
// component's uninterruptible writes and cleanup before it clears or reuses the
// same directories.

let tail: Promise<void> = Promise.resolve();

/** Acquire exclusive ownership of the builder's OPFS map-work directories. */
export async function acquireMapWorkStorage(): Promise<() => void> {
    const previous = tail;
    let unlock!: () => void;
    tail = new Promise<void>((resolve) => (unlock = resolve));
    await previous;
    let released = false;
    return () => {
        if (released) return;
        released = true;
        unlock();
    };
}
