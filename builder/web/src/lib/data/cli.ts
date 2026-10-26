// The page acts only through `POST /api/cli {"args": [...]}`, and sets a credential through
// `POST /api/credential {"args", "values"}`. The server runs `obc data --json ARGS` and answers
// every request, a refusal too, with `{exit, output}`: `output` is the JSON that the
// command wrote, and a failed command writes `{"error": {code, message, fix, run?}}`.

/** Why a command failed, and what to do about it: the CLI's `--json` error. */
export interface CliError {
    code: string;
    message: string;
    fix: string;
    run?: string;
}

export type CliResult<T> = { ok: true; output: T } | { ok: false; error: CliError };

const PLAIN = /^[\w@%+=:,./-]+$/;

// A long job starts a detached run, so every command answers in seconds. A request with no answer
// must still end: a poll asks again only after an answer, and a button stays busy until one.
const ANSWER_MS = 120_000;

/** The command a person types for `args`. The server adds `--json`, so `args` never holds it. */
export function command(args: readonly string[]): string {
    return ["obc", "data", ...args].map((arg) => (PLAIN.test(arg) ? arg : `'${arg.replaceAll("'", `'\\''`)}'`)).join(" ");
}

function unreachable(message: string): CliResult<never> {
    return {
        ok: false,
        error: { code: "failed", message, fix: "Start `obc data ui` on this machine again, then reload the page." },
    };
}

export function runCli<T>(args: readonly string[]): Promise<CliResult<T>> {
    if (args.includes("--json")) throw new Error("the server adds --json");
    return post("/api/cli", { args }, args);
}

/** Sets a credential with `args`, `source credential SOURCE` or `deploy credential WHAT`. The
 *  server gives `values` to that command on its standard input, so no value is in argv, and the
 *  answer never holds one. */
export function setCredential<T>(args: readonly string[], values: readonly string[]): Promise<CliResult<T>> {
    return post("/api/credential", { args, values }, args);
}

async function post<T>(route: string, body: object, args: readonly string[]): Promise<CliResult<T>> {
    let response: Response;
    try {
        response = await fetch(route, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(body),
            signal: AbortSignal.timeout(ANSWER_MS),
        });
    } catch (error) {
        return unreachable(
            (error as Error)?.name === "TimeoutError"
                ? "`obc data ui` gave no answer within 2 min"
                : "the page cannot reach `obc data ui`",
        );
    }
    const answer = (await response.json().catch(() => null)) as { exit?: unknown; output?: { error?: CliError } } | null;
    if (typeof answer?.exit !== "number") return unreachable(`\`obc data ui\` answered HTTP ${response.status} with no result`);
    if (answer.exit === 0) return { ok: true, output: answer.output as T };
    return {
        ok: false,
        error: answer.output?.error ?? {
            code: "failed",
            message: `\`${command(args)}\` stopped with exit status ${answer.exit} and wrote no error`,
            fix: "Run the command in a terminal to see its output.",
        },
    };
}
