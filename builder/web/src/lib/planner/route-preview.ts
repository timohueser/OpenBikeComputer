/** Keep drag previews outside saved history. Only the latest stationary position can publish. */
export function routePreview<T, R>(calculate: (value: T, signal: AbortSignal) => Promise<R>, publish: (value: T, result: R) => void, fail: (error: unknown) => void) {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let request: AbortController | undefined;
    function cancel() {
        clearTimeout(timer);
        request?.abort();
    }
    return {
        cancel,
        move(value: T) {
            cancel();
            const abort = new AbortController();
            request = abort;
            timer = setTimeout(() => {
                calculate(value, abort.signal).then(result => {
                    if (!abort.signal.aborted) publish(value, result);
                }).catch(error => { if (!abort.signal.aborted) fail(error); });
            }, 250);
        },
    };
}
