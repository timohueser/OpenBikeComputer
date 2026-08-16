/**
 * The metadata envelope codec, Device_Object_Protocol_v3.md §2.2 plus the registry's schemas.
 *
 * The envelope is the one place a domain adds a bounded declared fact without touching the wire
 * contract, so it is also the one place with two independent layers of rules: a *canonical form*
 * (strictly increasing unique base tags, an exact field-byte sum, no padding) and a *schema* (which
 * tags exist, how wide each is, what values each admits, which are required, and what a decoder
 * does with one it does not know). They report different things — the structural faults are
 * `noncanonicalMetadata`, `duplicateField` and `outOfOrderField`, and the schema faults are
 * `invalidCombination`, `schemaVersion` and `logicalKind` — and §2.2 says outright that a decoder
 * reporting the wrong one is nonconforming.
 *
 * **The order between those layers is normative**, because an envelope that breaks more than one
 * rule must still report one deterministic error: "canonical form first, then the schema's field
 * rules (identity, version, required/optional, widths, ranges, text validity), and the per-kind
 * registered maximum last. An envelope is measured against that maximum only after its fields
 * validate, so an unknown critical field in an oversized envelope reports the field error, not the
 * size." Every check below is placed to satisfy that sequence, and the sequence is what the
 * doubly-invalid unit vectors in `codec.test.ts` pin.
 */

import { Cursor, Writer } from "./bytes";
import {
    ENVELOPE_CEILING,
    ENVELOPE_HEADER_BYTES,
    OBJECT_KIND_NAME,
    SCHEMA_ROLE_OF_VERSION,
    metadataSchema,
    type MetadataFieldSpec,
    type MetadataSchema,
    type ObjectKindName,
    type SchemaRole,
} from "./registry";
import { decoding, reject, type DosResult } from "./result";

export const CRITICAL_BIT = 0x8000;
export const BASE_TAG_MASK = 0x7fff;

/** One encoded field, kept verbatim so an envelope re-encodes byte for byte. */
export interface MetadataField {
    readonly tag: number;
    readonly critical: boolean;
    readonly baseTag: number;
    readonly value: Uint8Array;
}

export type MetadataValue = number | bigint | boolean | string | Uint8Array;

export interface MetadataEnvelope {
    readonly schemaId: number;
    readonly schemaVersion: number;
    readonly kind: ObjectKindName;
    readonly role: SchemaRole;
    readonly fields: readonly MetadataField[];
    /** Named values for every field this schema knows. Unknown noncritical fields are not here. */
    readonly values: ReadonlyMap<string, MetadataValue>;
    /** Total encoded length, `8 + encoded_field_bytes`. */
    readonly byteLength: number;
}

export interface EnvelopeContext {
    /** The ObjectKind of the containing message; `schema_id` must match it exactly. */
    readonly kind?: ObjectKindName;
    /** The operation's schema role; the version byte must be the registered constant for it. */
    readonly role?: SchemaRole;
    /**
     * Mutating requests reject every unknown field, critical or not. Response projections reject an
     * unknown critical field and may skip a well-formed unknown noncritical one.
     */
    readonly mutating: boolean;
}

/** Total entry point. The throwing reader below is what the message decoders call. */
export const decodeMetadataEnvelope = (bytes: Uint8Array, context: EnvelopeContext): DosResult<MetadataEnvelope> =>
    decoding(() => readMetadataEnvelope(bytes, context));

export function readMetadataEnvelope(bytes: Uint8Array, context: EnvelopeContext): MetadataEnvelope {
    const head = new Cursor(bytes, { category: "invalidDescriptor", detail: "nestedLength" });
    const schemaId = head.u16();
    const schemaVersion = head.u8();
    const flags = head.u8();
    const encodedFieldBytes = head.u16();
    const fieldCount = head.u16();
    const byteLength = ENVELOPE_HEADER_BYTES + encodedFieldBytes;

    // --- canonical form -----------------------------------------------------------------------
    // Everything the §2.2 canonical-form paragraph governs, and nothing else: the header's own
    // reserved flags, then the field body read against the length the header declares. None of it
    // needs to know which schema this is, which is precisely why it comes first.
    if (flags !== 0) reject("invalidDescriptor", "reservedBits", "metadata envelope header flags are zero");
    const fields = readFields(bytes.subarray(ENVELOPE_HEADER_BYTES, byteLength), fieldCount);

    // --- schema field rules -------------------------------------------------------------------
    const role = SCHEMA_ROLE_OF_VERSION.get(schemaVersion);
    if (role === undefined) {
        reject("unsupportedCapability", "schemaVersion", `schema version ${schemaVersion} is not registered`);
    }
    if (context.role !== undefined && role !== context.role) {
        reject(
            "unsupportedCapability",
            "schemaVersion",
            `this operation carries a ${context.role} envelope, not a ${role} one`,
        );
    }
    // The common ceiling is a property of the role, so it can only be applied once the role is
    // known; the same is true of the availability check, which needs the declared boundary to be
    // plausible before it means anything.
    if (byteLength > ENVELOPE_CEILING[role]) {
        reject(
            "invalidDescriptor",
            "nestedLength",
            `a ${role} envelope is at most ${ENVELOPE_CEILING[role]} bytes, this one declares ${byteLength}`,
        );
    }
    if (bytes.length < byteLength) {
        reject("invalidDescriptor", "nestedLength", "the metadata envelope runs past its containing message");
    }

    const kind = OBJECT_KIND_NAME.get(schemaId);
    if (context.kind !== undefined && kind !== context.kind) {
        reject(
            "invalidDescriptor",
            "invalidCombination",
            `schema_id ${schemaId} does not match the containing ObjectKind`,
        );
    }
    if (kind === undefined) {
        reject("unsupportedCapability", "logicalKind", `schema_id ${schemaId} is not a registered ObjectKind`);
    }
    const schema = metadataSchema(kind, role);
    if (schema === undefined) {
        reject("unsupportedCapability", "logicalKind", `${kind} has no ${role} schema`);
    }
    const values = applySchema(fields, schema, context.mutating);

    // --- the per-kind registered maximum, last ------------------------------------------------
    if (byteLength > schema.maxBytes) {
        reject(
            "invalidDescriptor",
            "nestedLength",
            `the ${kind} ${role} schema is at most ${schema.maxBytes} bytes, this envelope declares ${byteLength}`,
        );
    }
    return { schemaId, schemaVersion, kind, role, fields, values, byteLength };
}

function readFields(body: Uint8Array, declaredCount: number): MetadataField[] {
    const fields: MetadataField[] = [];
    const view = new DataView(body.buffer, body.byteOffset, body.byteLength);
    let at = 0;
    let previousBaseTag = 0;
    while (at < body.length) {
        if (body.length - at < 4) {
            reject("invalidDescriptor", "noncanonicalMetadata", "a field header runs past the encoded field bytes");
        }
        const tag = view.getUint16(at, true);
        const valueLength = view.getUint16(at + 2, true);
        const baseTag = tag & BASE_TAG_MASK;
        if (baseTag === 0) {
            reject("invalidDescriptor", "noncanonicalMetadata", "the low 15 bits of a tag are a nonzero base tag");
        }
        if (fields.length > 0) {
            if (baseTag === previousBaseTag) {
                reject("invalidDescriptor", "duplicateField", `base tag ${baseTag} appears twice`);
            }
            if (baseTag < previousBaseTag) {
                reject("invalidDescriptor", "outOfOrderField", "fields are strictly increasing by base tag");
            }
        }
        if (body.length - at - 4 < valueLength) {
            reject(
                "invalidDescriptor",
                "noncanonicalMetadata",
                "encoded_field_bytes is the exact sum of every 4 + value_length",
            );
        }
        fields.push({
            tag,
            critical: (tag & CRITICAL_BIT) !== 0,
            baseTag,
            value: body.slice(at + 4, at + 4 + valueLength),
        });
        previousBaseTag = baseTag;
        at += 4 + valueLength;
    }
    if (fields.length !== declaredCount) {
        reject(
            "invalidDescriptor",
            "noncanonicalMetadata",
            `field_count says ${declaredCount} and the body carries ${fields.length}`,
        );
    }
    return fields;
}

function applySchema(
    fields: readonly MetadataField[],
    schema: MetadataSchema,
    mutating: boolean,
): ReadonlyMap<string, MetadataValue> {
    const byTag = new Map(schema.fields.map((spec) => [spec.tag, spec]));
    const values = new Map<string, MetadataValue>();
    for (const encoded of fields) {
        const spec = byTag.get(encoded.tag);
        if (spec === undefined) {
            if (mutating) {
                reject(
                    "invalidDescriptor",
                    "invalidCombination",
                    `a mutating request rejects the unknown field 0x${encoded.tag.toString(16)}`,
                );
            }
            if (encoded.critical) {
                reject(
                    "invalidDescriptor",
                    "invalidCombination",
                    `a projection rejects the unknown critical field 0x${encoded.tag.toString(16)}`,
                );
            }
            continue;
        }
        values.set(spec.name, decodeFieldValue(spec, encoded.value));
    }
    for (const spec of schema.fields) {
        if (spec.required && !values.has(spec.name)) {
            reject(
                "invalidDescriptor",
                "invalidCombination",
                `the ${schema.kind} ${schema.role} schema requires ${spec.name}`,
            );
        }
    }
    checkCrossFieldRules(schema, values);
    return values;
}

/**
 * The one registered rule that spans two fields. Registry §3 fixes the required valid-until time as
 * "later than earliest issued UTC", and a weather Put declares both — so each field is in range on
 * its own and only the pair is illegal, which is `invalidCombination` by construction. Equality is
 * refused with everything earlier: a bundle that expires the instant it was issued covers nothing.
 *
 * It runs after the per-field rules and before the registered maximum, which is the §2.2 order.
 */
function checkCrossFieldRules(schema: MetadataSchema, values: ReadonlyMap<string, MetadataValue>): void {
    if (schema.kind !== "weather" || schema.role !== "put") return;
    const issued = values.get("issuedUtc");
    const validUntil = values.get("validUntilUtc");
    if (typeof issued === "bigint" && typeof validUntil === "bigint" && validUntil <= issued) {
        reject(
            "invalidDescriptor",
            "invalidCombination",
            `validUntilUtc ${validUntil} is not later than issuedUtc ${issued}`,
        );
    }
}

function widthOf(spec: MetadataFieldSpec): number | undefined {
    switch (spec.type.kind) {
        case "u8":
        case "bool":
            return 1;
        case "u16":
            return 2;
        case "u32":
        case "i32":
            return 4;
        case "u64":
        case "i64":
            return 8;
        case "bytes":
            return spec.type.exact;
        case "text":
            return undefined;
    }
}

/**
 * A registered range is a field rule, not a courtesy. The two kinds fail differently: a value
 * outside an *enumeration* names no registered case, which is `unknownEnum`; a continuous quantity
 * outside its bounds is a legal number in an illegal place, which is the generic illegal-field-value
 * detail `invalidCombination`.
 */
function checkBounds(spec: MetadataFieldSpec, value: bigint): void {
    const bounds = spec.bounds;
    if (bounds === undefined || (value >= bounds.min && value <= bounds.max)) return;
    if (bounds.enumerated) {
        reject("invalidDescriptor", "unknownEnum", `${spec.name} value ${value} is not a registered case`);
    }
    reject(
        "invalidDescriptor",
        "invalidCombination",
        `${spec.name} is registered at ${bounds.min}..${bounds.max} and carries ${value}`,
    );
}

function decodeFieldValue(spec: MetadataFieldSpec, value: Uint8Array): MetadataValue {
    const width = widthOf(spec);
    if (width !== undefined && value.length !== width) {
        reject(
            "invalidDescriptor",
            "noncanonicalMetadata",
            `${spec.name} is registered at ${width} bytes and carries ${value.length}`,
        );
    }
    const view = new DataView(value.buffer, value.byteOffset, value.byteLength);
    const numeric = (raw: number | bigint): number | bigint => {
        checkBounds(spec, BigInt(raw));
        return raw;
    };
    switch (spec.type.kind) {
        case "u8":
            return numeric(view.getUint8(0));
        case "u16":
            return numeric(view.getUint16(0, true));
        case "u32":
            return numeric(view.getUint32(0, true));
        case "u64":
            return numeric(view.getBigUint64(0, true));
        case "i32":
            return numeric(view.getInt32(0, true));
        case "i64":
            return numeric(view.getBigInt64(0, true));
        case "bool": {
            const raw = view.getUint8(0);
            // §2.2 defines the boolean's *encoding* as the byte `0` or `1`, so a third value is a
            // noncanonical encoding rather than an unregistered member of a value space —
            // `noncanonicalMetadata`, the same detail an unclean text field earns, and not the
            // `unknownEnum` a registered enumeration answers with.
            if (raw > 1) reject("invalidDescriptor", "noncanonicalMetadata", `${spec.name} is encoded 0 or 1`);
            return raw === 1;
        }
        case "bytes":
            return value.slice();
        case "text": {
            if (value.length < spec.type.min || value.length > spec.type.max) {
                reject(
                    "invalidDescriptor",
                    "noncanonicalMetadata",
                    `${spec.name} is registered at ${spec.type.min}-${spec.type.max} bytes and carries ${value.length}`,
                );
            }
            return readWireText(value, spec.name);
        }
    }
}

export function writeMetadataEnvelope(envelope: MetadataEnvelope): Uint8Array {
    let encodedFieldBytes = 0;
    for (const encoded of envelope.fields) encodedFieldBytes += 4 + encoded.value.length;
    const writer = new Writer(ENVELOPE_HEADER_BYTES + encodedFieldBytes);
    writer.u16(envelope.schemaId).u8(envelope.schemaVersion).u8(0).u16(encodedFieldBytes).u16(envelope.fields.length);
    for (const encoded of envelope.fields) writer.u16(encoded.tag).u16(encoded.value.length).raw(encoded.value);
    return writer.finish();
}

/**
 * §2.2's text rule: shortest-form valid UTF-8 with no NUL, C0/C1 control, surrogate, or noncharacter
 * scalar. Accepted bytes are canonical as-is — no normalizing, trimming, or case folding — so this
 * validates and then decodes, and never rewrites.
 *
 * `TextDecoder` cannot stand in for it: it replaces bad sequences rather than reporting them, and
 * even in fatal mode it says nothing about C0 controls or noncharacters.
 */
export function readWireText(bytes: Uint8Array, what: string): string {
    const scalars: number[] = [];
    let at = 0;
    const bad = (why: string): never =>
        reject("invalidDescriptor", "noncanonicalMetadata", `${what} is not ${why} (offset ${at})`);
    while (at < bytes.length) {
        const lead = bytes[at];
        let scalar: number;
        let width: number;
        if (lead < 0x80) {
            scalar = lead;
            width = 1;
        } else if (lead >= 0xc2 && lead <= 0xdf) {
            scalar = lead & 0x1f;
            width = 2;
        } else if (lead >= 0xe0 && lead <= 0xef) {
            scalar = lead & 0x0f;
            width = 3;
        } else if (lead >= 0xf0 && lead <= 0xf4) {
            scalar = lead & 0x07;
            width = 4;
        } else {
            return bad("valid UTF-8");
        }
        if (at + width > bytes.length) return bad("valid UTF-8");
        for (let i = 1; i < width; i++) {
            const continuation = bytes[at + i];
            if ((continuation & 0xc0) !== 0x80) return bad("valid UTF-8");
            scalar = (scalar << 6) | (continuation & 0x3f);
        }
        // Shortest form, and the surrogate and above-U+10FFFF holes.
        if (width === 3 && scalar < 0x800) return bad("shortest-form UTF-8");
        if (width === 4 && (scalar < 0x10000 || scalar > 0x10ffff)) return bad("shortest-form UTF-8");
        if (scalar >= 0xd800 && scalar <= 0xdfff) return bad("free of surrogate scalars");
        if (scalar === 0) return bad("free of NUL");
        if (scalar < 0x20 || (scalar >= 0x7f && scalar <= 0x9f)) return bad("free of C0/C1 controls");
        if (scalar >= 0xfdd0 && scalar <= 0xfdef) return bad("free of noncharacters");
        if ((scalar & 0xfffe) === 0xfffe) return bad("free of noncharacters");
        scalars.push(scalar);
        at += width;
    }
    return String.fromCodePoint(...scalars);
}

/**
 * §12's rendering rule for diagnostic text: a receiver never rejects a frame over it, and renders
 * it lossily — dropping any sequence that is not valid, non-control, non-noncharacter UTF-8.
 */
export function renderDiagnosticText(bytes: Uint8Array): string {
    let out = "";
    let at = 0;
    while (at < bytes.length) {
        for (let width = Math.min(4, bytes.length - at); width >= 1; width--) {
            const slice = bytes.subarray(at, at + width);
            try {
                out += readWireText(slice, "text");
                at += width;
                break;
            } catch {
                if (width === 1) at += 1;
            }
        }
    }
    return out;
}
