import { describe, expect, it } from "vitest";
import generatedConfigSchema from "../../../../../host/obc-pack/schema/config.schema.json";
import {
    enumDisplayValue,
    resolveSchemaField,
    stringEnumOptions,
    type JsonSchema,
} from "./schema_fields";

const generated = generatedConfigSchema as unknown as JsonSchema;

function generatedStyleField(name: string): JsonSchema {
    const defs = generated.$defs as Record<string, JsonSchema>;
    const style = defs.style;
    return (style.properties as Record<string, JsonSchema>)[name];
}

function generatedProfileField(name: string): JsonSchema {
    const defs = generated.$defs as Record<string, JsonSchema>;
    const profile = defs.profile;
    return (profile.properties as Record<string, JsonSchema>)[name];
}

describe("generic schema field resolution", () => {
    it("renders the generated nullable line_style ref as the solid/dashed enum", () => {
        const raw = generatedStyleField("line_style");
        expect(raw.anyOf).toBeDefined(); // discriminates from the former inline-enum mock

        const resolved = resolveSchemaField(generated, raw);
        const options = stringEnumOptions(resolved.schema);
        expect(resolved.nullable).toBe(true);
        expect(options).toEqual(["solid", "dashed"]);
        expect(enumDisplayValue("dashed", resolved.schema, options!)).toBe("dashed");
        expect(enumDisplayValue(null, resolved.schema, options!)).toBe("solid");
        expect(enumDisplayValue("invalid", resolved.schema, options!)).toBe("solid");
    });

    it("resolves the generated color ref while preserving its field description", () => {
        const resolved = resolveSchemaField(generated, generatedStyleField("color2"));
        expect(resolved.schema.description).toMatch(/secondary RGB565/);
        expect(resolved.schema.oneOf).toEqual((generated.$defs as Record<string, JsonSchema>).color.oneOf);
    });

    it("normalizes generated nullable numeric type arrays", () => {
        const resolved = resolveSchemaField(generated, generatedStyleField("weight"));
        expect(resolved).toMatchObject({ nullable: true, schema: { type: "integer", minimum: 0, maximum: 255 } });
    });

    it("retains the other generated nullable ref's genuine multiplier union", () => {
        const resolved = resolveSchemaField(generated, generatedProfileField("default"));
        const multiplier = (generated.$defs as Record<string, JsonSchema>).multiplier;
        expect(resolved.nullable).toBe(true);
        expect(resolved.schema.oneOf).toEqual(multiplier.oneOf);
        expect(resolved.schema.default).toBe(2.0);
    });

    it("leaves unknown and cyclic refs unresolved without recursing forever", () => {
        expect(resolveSchemaField({}, { $ref: "#/$defs/missing" }).schema).toEqual({
            $ref: "#/$defs/missing",
        });

        const cyclic: JsonSchema = { $defs: { a: { $ref: "#/$defs/b" }, b: { $ref: "#/$defs/a" } } };
        expect(() => resolveSchemaField(cyclic, { $ref: "#/$defs/a" })).not.toThrow();
        expect(resolveSchemaField(cyclic, { $ref: "#/$defs/a" }).schema.$ref).toBeDefined();
    });
});
