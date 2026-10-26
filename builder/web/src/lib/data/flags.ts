// The settings of an update are flags of `obc data update`. The page sends them with the next
// `--plan`: it has no other way to change settings.

const SETTINGS = ["--region", "--box", "--add", "--remove", "--refresh", "--only"];

export type Flags = [flag: string, value: string][];

/** The settings flags of `args`, without `update TARGET`, `--expect ID` and the other words. */
export function flagsOf(args: readonly string[]): Flags {
    const flags: Flags = [];
    for (let i = 0; i < args.length - 1; i++) {
        if (SETTINGS.includes(args[i])) flags.push([args[i], args[++i]]);
    }
    return flags;
}

/** The review of an update of `target` with `flags`. */
export function reviewArgs(target: string, flags: Flags): string[] {
    return ["update", target, ...flags.flat(), "--dry-run"];
}

/** The run that plans an update of `target` with `flags`. */
export function planArgs(target: string, flags: Flags): string[] {
    return ["update", target, ...flags.flat(), "--plan"];
}
