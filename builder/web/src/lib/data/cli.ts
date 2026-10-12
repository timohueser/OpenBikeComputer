// The page acts only through `POST /api/cli {"args": [...]}`. The server runs `obc data ARGS --json`
// and answers every request, a refusal too, with `{exit, output}`: `output` is the JSON that the
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

export async function runCli<T>(args: readonly string[]): Promise<CliResult<T>> {
    if (args.includes("--json")) throw new Error("the server adds --json");
    let response: Response;
    try {
        response = await fetch("/api/cli", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ args }),
        });
    } catch {
        return unreachable("the page cannot reach `obc data ui`");
    }
    const body = (await response.json().catch(() => null)) as { exit?: unknown; output?: { error?: CliError } } | null;
    if (typeof body?.exit !== "number") return unreachable(`\`obc data ui\` answered HTTP ${response.status} with no result`);
    if (body.exit === 0) return { ok: true, output: body.output as T };
    return {
        ok: false,
        error: body.output?.error ?? {
            code: "failed",
            message: `\`${command(args)}\` stopped with exit status ${body.exit} and wrote no error`,
            fix: "Run the command in a terminal to see its output.",
        },
    };
}
