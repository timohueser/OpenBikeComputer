import { readFileSync } from "node:fs";
import { expect, it } from "vitest";

// Each example that the page tests read has the shape that the spec gives for its command: the
// spec's JSON schemas come from the Rust types of `obc data`.
const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8");
const spec = read("../../../../../specs/obc-data.md");
const block = spec.slice(spec.indexOf("## JSON schemas"));
const start = block.indexOf("```json") + "```json".length;
const defs: Record<string, Schema> = JSON.parse(block.slice(start, block.indexOf("```", start))).$defs;

interface Schema {
    $ref?: string;
    type?: string | string[];
    enum?: unknown[];
    const?: unknown;
    anyOf?: Schema[];
    oneOf?: Schema[];
    properties?: Record<string, Schema>;
    required?: string[];
    additionalProperties?: boolean | Schema;
    items?: Schema;
    prefixItems?: Schema[];
}

function kind(value: unknown): string {
    if (value === null) return "null";
    if (Array.isArray(value)) return "array";
    return Number.isInteger(value) ? "integer" : typeof value;
}

/** Where `value` breaks `schema`: the subset of JSON Schema that schemars writes. */
function errors(value: unknown, schema: Schema, at: string): string[] {
    if (schema.$ref) return errors(value, defs[schema.$ref.replace("#/$defs/", "")], at);
    if (schema.anyOf) return schema.anyOf.some((one) => !errors(value, one, at).length) ? [] : [`${at}: no anyOf`];
    if (schema.oneOf) return schema.oneOf.filter((one) => !errors(value, one, at).length).length === 1 ? [] : [`${at}: not one oneOf`];
    if ("const" in schema && value !== schema.const) return [`${at}: not ${schema.const}`];
    if (schema.enum && !schema.enum.includes(value)) return [`${at}: ${value} is not one of ${schema.enum}`];
    const types = [schema.type ?? []].flat();
    const type = kind(value);
    if (types.length && !types.includes(type) && !(type === "integer" && types.includes("number"))) return [`${at}: ${type}, not ${types}`];
    if (Array.isArray(value)) {
        return value.flatMap((item, i) => {
            const each = schema.prefixItems?.[i] ?? schema.items;
            return each ? errors(item, each, `${at}[${i}]`) : [];
        });
    }
    if (type !== "object") return [];
    const object = value as Record<string, unknown>;
    const missing = (schema.required ?? []).filter((key) => !(key in object)).map((key) => `${at}.${key}: missing`);
    return missing.concat(
        Object.entries(object).flatMap(([key, field]) => {
            const each = schema.properties?.[key] ?? schema.additionalProperties;
            if (each === false) return [`${at}.${key}: unknown`];
            return typeof each === "object" ? errors(field, each, `${at}.${key}`) : [];
        }),
    );
}

const EXAMPLES = {
    "status.json": "Overview",
    "setup.json": "Setup",
    "runs.json": "RunList",
    "run-running.json": "View",
    "run-failed.json": "View",
};

it.each(Object.entries(EXAMPLES))("the example %s has the schema %s of the spec", (name, schema) => {
    const example = JSON.parse(read(`../../../test-support/data/${name}`));
    expect(errors(example, defs[schema], "$")).toEqual([]);
});

it("finds a missing key, a wrong value and an unknown key", () => {
    const machine = { name: "laptop", colour: "#262624", os: "macos", cores: "ten", memory_bytes: null, disk: 1 };
    expect(errors(machine, defs.Machine, "$")).toEqual([
        "$.store_free_bytes: missing",
        "$.cores: string, not integer",
        "$.disk: unknown",
    ]);
});
