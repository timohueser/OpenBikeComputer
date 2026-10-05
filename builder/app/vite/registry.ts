import fs from "node:fs";

/** The `attribution` of one source in data/sources.toml, the one home of every credit. */
export function registryAttribution(id: string): string {
    const registry = fs.readFileSync(new URL("../../../data/sources.toml", import.meta.url), "utf8");
    const entry = registry.split("[[source]]").find((block) => block.includes(`\nid = "${id}"\n`)) ?? "";
    const attribution = /\nattribution = "([^"]*)"/.exec(entry)?.[1];
    if (!attribution) throw new Error(`data/sources.toml has no attribution for ${id}`);
    return attribution;
}
