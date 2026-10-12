import { runCli, type CliError, type CliResult } from "./cli";

/** The answers to one read-only command, shared by every view that shows them. */
export class CliQuery<T> {
    /** The output of the latest answer that succeeded. A failed answer keeps it. */
    output = $state<T | null>(null);
    /** When `output` came. */
    at = $state<Date | null>(null);
    /** The error of the latest answer, or null when it succeeded. */
    error = $state<CliError | null>(null);
    #asked = 0;

    constructor(readonly args: readonly string[]) {}

    /** Starts an ask and returns the function that takes its answer. An answer is dropped when a
     *  later ask started before it came, so an older answer never overwrites a newer one. */
    ask(): (result: CliResult<T>) => void {
        const asked = ++this.#asked;
        return (result) => {
            if (asked !== this.#asked) return;
            this.error = result.ok ? null : result.error;
            if (!result.ok) return;
            this.output = result.output;
            this.at = new Date();
        };
    }

    async refresh() {
        const take = this.ask();
        take(await runCli<T>(this.args));
    }

    /** Refreshes now, and again `ms` after each answer, until the returned function is called. */
    poll(ms: number): () => void {
        let timer: ReturnType<typeof setTimeout> | undefined;
        let stopped = false;
        const next = async () => {
            await this.refresh();
            if (!stopped) timer = setTimeout(next, ms);
        };
        void next();
        return () => {
            stopped = true;
            clearTimeout(timer);
        };
    }
}
