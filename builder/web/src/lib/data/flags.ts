// The settings of an update are flags of `obc data update`. The page keeps the edits of the settings
// as these flags and sends them with the next `--plan`: it has no other way to change settings.

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

/** `flags` in the order in which `obc data update` gives them back in the arguments of its run. */
export function canonical(flags: Flags): Flags {
    return [...flags].sort(([a], [b]) => SETTINGS.indexOf(a) - SETTINGS.indexOf(b));
}

/** `flags` without the flags that `old` matches, and with `flag` last when there is one. */
export function edit(flags: Flags, old: (flag: string, value: string) => boolean, flag?: [string, string]): Flags {
    return [...flags.filter(([name, value]) => !old(name, value)), ...(flag ? [flag] : [])];
}

/** The review of an update of `target` with `flags`. */
export function reviewArgs(target: string, flags: Flags): string[] {
    return ["update", target, ...flags.flat(), "--dry-run"];
}

/** The run that plans an update of `target` with `flags`. */
export function planArgs(target: string, flags: Flags): string[] {
    return ["update", target, ...flags.flat(), "--plan"];
}
