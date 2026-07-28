// Defensive resolution for fields rendered by the advanced editor's generic
// SchemaField component. obc-pack owns the schema; this module only turns the
// common JSON Schema spellings it emits ($ref, nullable unions/type arrays)
// into the single field shape the control needs.

export type JsonSchema = Record<string, unknown>;

export interface ResolvedSchemaField {
    schema: JsonSchema;
    nullable: boolean;
}

const MAX_RESOLVE_DEPTH = 32;

function isSchema(value: unknown): value is JsonSchema {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

function localRef(root: JsonSchema, ref: string): JsonSchema | null {
    if (!ref.startsWith("#/")) return null;
    let cursor: unknown = root;
    for (const encoded of ref.slice(2).split("/")) {
        let segment: string;
        try {
            segment = decodeURIComponent(encoded).replace(/~1/g, "/").replace(/~0/g, "~");
        } catch {
            return null;
        }
        if (!isSchema(cursor) || !Object.prototype.hasOwnProperty.call(cursor, segment)) return null;
        cursor = cursor[segment];
    }
    return isSchema(cursor) ? cursor : null;
}

function isNullSchema(schema: JsonSchema): boolean {
    if (schema.type === "null" || schema.const === null) return true;
    return Array.isArray(schema.enum) && schema.enum.length === 1 && schema.enum[0] === null;
}

function resolveNode(
    root: JsonSchema,
    input: JsonSchema,
    activeRefs: Set<string>,
    depth: number,
): ResolvedSchemaField {
    if (depth >= MAX_RESOLVE_DEPTH) return { schema: { ...input }, nullable: false };

    let schema: JsonSchema = { ...input };
    let nullable = false;

    const ref = schema.$ref;
    if (typeof ref === "string" && !activeRefs.has(ref)) {
        const target = localRef(root, ref);
        if (target) {
            activeRefs.add(ref);
            const resolved = resolveNode(root, target, activeRefs, depth + 1);
            activeRefs.delete(ref);
            const siblings = { ...schema };
            delete siblings.$ref;
            schema = { ...resolved.schema, ...siblings };
            nullable ||= resolved.nullable;
        }
    }

    for (const unionKey of ["anyOf", "oneOf"] as const) {
        const variants = schema[unionKey];
        if (!Array.isArray(variants) || !variants.every(isSchema)) continue;
        const resolved = variants.map((variant) => resolveNode(root, variant, activeRefs, depth + 1));
        const concrete = resolved.filter((variant) => !isNullSchema(variant.schema));
        const nullCount = resolved.length - concrete.length;
        nullable ||= nullCount > 0 || resolved.some((variant) => variant.nullable);

        // `T | null` is the shape schemars emits for Option<T>. Collapse only
        // that unambiguous case; a genuine multi-shape union remains a union.
        if (nullCount > 0 && concrete.length === 1) {
            const parent = { ...schema };
            delete parent[unionKey];
            schema = { ...concrete[0].schema, ...parent };
        } else {
            schema[unionKey] = resolved.map((variant) => variant.schema);
        }
    }

    if (Array.isArray(schema.type)) {
        const types = schema.type.filter((type): type is string => typeof type === "string");
        const concrete = types.filter((type) => type !== "null");
        nullable ||= concrete.length !== types.length;
        if (concrete.length === 1) schema.type = concrete[0];
        else if (concrete.length > 1) schema.type = concrete;
    }

    return { schema, nullable };
}

/** Resolve one schema field without throwing on unknown, external, or cyclic refs. */
export function resolveSchemaField(root: JsonSchema, field: JsonSchema): ResolvedSchemaField {
    return resolveNode(root, field, new Set<string>(), 0);
}

/** String options for a select control, or null when this is not a string enum. */
export function stringEnumOptions(schema: JsonSchema): string[] | null {
    if (!Array.isArray(schema.enum) || !schema.enum.every((value) => typeof value === "string")) return null;
    return schema.enum;
}

/** Stable select value: null/unknown values display the schema default or first option. */
export function enumDisplayValue(value: unknown, schema: JsonSchema, options: string[]): string {
    if (typeof value === "string" && options.includes(value)) return value;
    if (typeof schema.default === "string" && options.includes(schema.default)) return schema.default;
    return options[0] ?? "";
}
