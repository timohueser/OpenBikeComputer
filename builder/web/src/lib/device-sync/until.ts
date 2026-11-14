/** The result of `work`, or the reason of `signal` when it aborts first. `work` itself goes on: this only ends the wait for
 * a step that takes no signal, such as a module import. */
export function until<T>(work: Promise<T>, signal: AbortSignal): Promise<T> {
    return new Promise<T>((resolve, reject) => {
        if (signal.aborted) return reject(signal.reason);
        signal.addEventListener('abort', () => reject(signal.reason), { once: true });
        work.then(resolve, reject);
    });
}
