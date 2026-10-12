import { runCli, type CliResult } from "./cli";

/** The latest result of one read-only command, shared by every view that shows it. */
export class CliQuery<T> {
    result = $state<CliResult<T> | null>(null);
    /** The output of the latest result, or null while there is none or it is an error. */
    output = $derived(this.result?.ok ? this.result.output : null);
    at = $state<Date | null>(null);
    #asked = 0;

    constructor(readonly args: readonly string[]) {}

    /** Takes a newer result, such as the one an `<Action>` with the same args returned. A refresh
     *  that is still in flight then does not overwrite it. */
    accept(result: CliResult<T>) {
        this.#asked++;
        this.result = result;
        this.at = new Date();
    }

    async refresh() {
        const asked = ++this.#asked;
        const result = await runCli<T>(this.args);
        if (asked !== this.#asked) return;
        this.result = result;
        this.at = new Date();
    }

    /** Refreshes now and then every `ms` until the returned function is called. */
    poll(ms: number): () => void {
        void this.refresh();
        const timer = setInterval(() => void this.refresh(), ms);
        return () => clearInterval(timer);
    }
}
