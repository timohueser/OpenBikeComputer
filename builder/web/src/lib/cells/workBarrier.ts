// OPFS map work is origin-global, while a map run belongs to one component of one tab. A remount, the map send of
// the planner and a second tab must therefore wait for the uninterruptible writes and the cleanup of the owner before
// them, before they clear or reuse the same directories. The Web Lock orders them in the whole browser.

const LOCK = "obc-map-work";

let tail: Promise<void> = Promise.resolve();

/** The order of one tab, for a host with no Web Locks. */
async function inTab(): Promise<() => void> {
    const previous = tail;
    let unlock!: () => void;
    tail = new Promise<void>((resolve) => (unlock = resolve));
    await previous;
    return unlock;
}

/** Acquire exclusive ownership of the OPFS map-work directories. `signal` ends the wait with its reason. */
export async function acquireMapWorkStorage(signal?: AbortSignal): Promise<() => void> {
    const locks = globalThis.navigator?.locks;
    const unlock = locks
        ? await new Promise<() => void>((resolve, reject) => {
              // The lock is held until the promise of the callback resolves.
              locks.request(LOCK, { signal }, () => new Promise<void>((release) => resolve(release))).catch(reject);
          })
        : await inTab();
    let released = false;
    return () => {
        if (released) return;
        released = true;
        unlock();
    };
}

/** Whether another owner holds the map-work directories now, in this tab or in another one. */
export async function mapWorkBusy(): Promise<boolean> {
    const state = await globalThis.navigator?.locks?.query();
    return !!state?.held?.some((lock) => lock.name === LOCK);
}
