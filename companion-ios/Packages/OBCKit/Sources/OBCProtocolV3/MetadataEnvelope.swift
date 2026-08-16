import Foundation

/// A decoded metadata field. Values are kept in their registered wire shape; nothing is normalized.
public enum MetadataValue: Hashable, Sendable {
    case u8(UInt8)
    case u16(UInt16)
    case u32(UInt32)
    case u64(UInt64)
    case i32(Int32)
    case i64(Int64)
    case boolean(Bool)
    case text(String)
    case bytes([UInt8])
}

/// One `tag u16, value_length u16, value` field of §2.2. The tag's high bit is the critical bit and
/// its low 15 bits are a nonzero base tag.
public struct MetadataField: Hashable, Sendable {
    public let baseTag: UInt16
    public let critical: Bool
    public let value: [UInt8]

    public var tag: UInt16 { critical ? baseTag | 0x8000 : baseTag }
    public var encodedLength: Int { 4 + value.count }
}

/// The registry-governed metadata envelope of §2.2. It is the one place a domain adds a bounded
/// declared fact without touching the common wire contract.
public struct MetadataEnvelope: Hashable, Sendable {
    public let schemaId: UInt16
    public let schemaVersion: UInt8
    public let fields: [MetadataField]

    public var schemaClass: SchemaClass? { SchemaClass(version: schemaVersion) }
    public var encodedFieldBytes: Int { fields.reduce(0) { $0 + $1.encodedLength } }
    /// §2.2: total length is `8 + encoded_field_bytes`.
    public var encodedLength: Int { 8 + encodedFieldBytes }

    public init(schemaId: UInt16, schemaVersion: UInt8, fields: [MetadataField]) {
        self.schemaId = schemaId
        self.schemaVersion = schemaVersion
        self.fields = fields
    }

    // MARK: decode

    /// Canonical-form decode only: header shape, canonical field encoding, ordering, and the class
    /// ceiling — which is a **call-site** fact, not something read off the wire.
    ///
    /// §2.2 fixes canonical form as the *first* validation stage, so nothing here may consult the
    /// schema registry. In particular the version byte is copied through unexamined: deciding it is
    /// unregistered is a schema-field rule and belongs to `validated(kind:schemaClass:mutating:)`.
    /// Deriving `maximumEncodedLength` from the version instead of from the caller would smuggle
    /// the same schema decision back in — an envelope that lies about its version would then be
    /// measured against the ceiling it claims rather than the one its position in the message
    /// imposes, and would report a size error where the contract requires a version error.
    public static func decode(_ bytes: [UInt8], maximumEncodedLength: Int) throws -> MetadataEnvelope
    {
        var reader = ByteReader(bytes, subject: "metadata envelope")
        let schemaId = try reader.u16()
        let schemaVersion = try reader.u8()
        let flags = try reader.u8()
        guard flags == 0 else { throw WireFault.reservedBits("metadata envelope: header flags") }
        let encodedFieldBytes = Int(try reader.u16())
        let fieldCount = Int(try reader.u16())

        // Checked before the body length, so an envelope that merely *claims* to exceed its class
        // ceiling is rejected as a nested-length error rather than as truncation.
        guard 8 + encodedFieldBytes <= maximumEncodedLength else {
            throw WireFault.nestedLength(
                "metadata envelope: \(8 + encodedFieldBytes) over the ceiling \(maximumEncodedLength)")
        }
        guard bytes.count == 8 + encodedFieldBytes else {
            throw WireFault.noncanonicalMetadata(
                "metadata envelope: \(bytes.count) bytes for a declared \(8 + encodedFieldBytes)")
        }

        var fields: [MetadataField] = []
        var seen: Set<UInt16> = []
        var previousBase: UInt16?
        var consumed = 0
        while consumed < encodedFieldBytes {
            guard encodedFieldBytes - consumed >= 4 else {
                throw WireFault.noncanonicalMetadata("metadata envelope: field header runs past the body")
            }
            let tag = try reader.u16()
            let valueLength = Int(try reader.u16())
            let baseTag = tag & 0x7FFF
            guard baseTag != 0 else {
                throw WireFault.noncanonicalMetadata("metadata envelope: zero base tag")
            }
            guard consumed + 4 + valueLength <= encodedFieldBytes else {
                throw WireFault.noncanonicalMetadata(
                    "metadata envelope: value_length \(valueLength) runs past the body")
            }
            let value = Array(try reader.take(valueLength))
            if seen.contains(baseTag) {
                throw WireFault.duplicateField("metadata envelope: base tag \(baseTag)")
            }
            if let previous = previousBase, baseTag < previous {
                throw WireFault.outOfOrderField("metadata envelope: base tag \(baseTag) after \(previous)")
            }
            seen.insert(baseTag)
            previousBase = baseTag
            fields.append(
                MetadataField(baseTag: baseTag, critical: tag & 0x8000 != 0, value: value))
            consumed += 4 + valueLength
        }
        guard consumed == encodedFieldBytes else {
            throw WireFault.noncanonicalMetadata("metadata envelope: field sum \(consumed) != \(encodedFieldBytes)")
        }
        guard fields.count == fieldCount else {
            throw WireFault.noncanonicalMetadata(
                "metadata envelope: field_count \(fieldCount) != \(fields.count) fields")
        }
        return MetadataEnvelope(schemaId: schemaId, schemaVersion: schemaVersion, fields: fields)
    }

    /// Fallible, like every encoder here: §1 forbids emitting a structure that cannot be framed, so
    /// a field body or field count that will not fit its `u16` is refused rather than truncated.
    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.u16(schemaId)
        writer.u8(schemaVersion)
        writer.u8(0)
        writer.u16(try narrowU16(encodedFieldBytes, "metadata envelope: encoded field bytes"))
        writer.u16(try narrowU16(fields.count, "metadata envelope: field count"))
        for field in fields {
            writer.u16(field.tag)
            writer.u16(try narrowU16(field.value.count, "metadata envelope: value length"))
            writer.raw(field.value)
        }
        return writer.bytes
    }

    // MARK: schema validation

    /// §2.2 + registries §4. `mutating` selects the two rejection regimes: a Put or patch request
    /// rejects every unknown field, critical or not, while a response projection rejects only
    /// unknown *critical* fields and may skip a well-formed unknown noncritical one.
    ///
    /// §2.2 fixes the order so a multi-fault envelope reports one deterministic error: canonical
    /// form first (`decode` above), then the schema's field rules — identity, version,
    /// required/optional, widths, ranges, text validity — and the per-kind registered maximum
    /// **last**. "An envelope is measured against that maximum only after its fields validate, so
    /// an unknown critical field in an oversized envelope reports the field error, not the size."
    @discardableResult
    public func validated(
        kind: ObjectKind, schemaClass expected: SchemaClass, mutating: Bool
    ) throws -> [UInt16: MetadataValue] {
        guard schemaId == kind.rawValue else {
            throw WireFault.invalidCombination(
                "metadata envelope: schema_id \(schemaId) is not ObjectKind \(kind.rawValue)")
        }
        // Both an unregistered version and a registered-but-wrong one are the same fact here: this
        // envelope does not carry the schema its position demands. Registries §4 makes the three
        // version numbers constants rather than a negotiation, so there is nothing to fall back to.
        guard schemaVersion == expected.version else {
            throw WireFault.unsupportedSchemaVersion(
                "metadata envelope: version \(schemaVersion), expected \(expected.version)")
        }
        guard let schema = MetadataRegistry.schema(kind: kind, schemaClass: expected) else {
            throw WireFault.unsupportedLogicalKind(
                "\(kind.name) has no \(expected) schema")
        }

        var decoded: [UInt16: MetadataValue] = [:]
        for field in fields {
            guard let spec = schema.fields.first(where: { $0.baseTag == field.baseTag }) else {
                if mutating || field.critical {
                    throw WireFault.invalidCombination(
                        "\(kind.name): unknown \(field.critical ? "critical" : "noncritical") tag \(field.baseTag)")
                }
                continue  // a projection may skip a well-formed unknown noncritical field
            }
            guard spec.critical == field.critical else {
                throw WireFault.invalidCombination(
                    "\(kind.name): tag \(field.baseTag) has the wrong critical bit")
            }
            decoded[field.baseTag] = try spec.type.decode(
                field.value, subject: "\(kind.name) tag \(field.baseTag)")
        }
        for spec in schema.fields where spec.required {
            guard decoded[spec.baseTag] != nil else {
                throw WireFault.invalidCombination(
                    "\(kind.name): required tag \(spec.baseTag) missing")
            }
        }
        for rule in schema.crossFieldRules {
            try rule.validate(decoded, kind: kind)
        }
        // Registries §4: "A decoder rejects a schema-specific envelope larger than its value above
        // even though the common ceilings are 128 and 96." Checked last so an unknown critical
        // field — the reason the envelope is oversized in the first place — is reported as itself.
        guard encodedLength <= schema.maximumEncodedLength else {
            throw WireFault.nestedLength(
                "\(kind.name) \(expected): \(encodedLength) over the registered \(schema.maximumEncodedLength)")
        }
        return decoded
    }
}

/// A rule spanning two fields of one envelope, applied after every individual field validates.
public enum CrossFieldRule: Sendable {
    /// The `later` `i64` must be strictly greater than the `earlier` one.
    case strictlyLaterI64(earlier: UInt16, later: UInt16, what: String)

    func validate(_ decoded: [UInt16: MetadataValue], kind: ObjectKind) throws {
        switch self {
        case .strictlyLaterI64(let earlier, let later, let what):
            guard case .i64(let a)? = decoded[earlier], case .i64(let b)? = decoded[later] else {
                return  // a missing field is already the required-field rule's business
            }
            // A continuous quantity out of bounds, so `invalidCombination` rather than `unknownEnum`.
            guard b > a else {
                throw WireFault.invalidCombination("\(kind.name): \(what) (\(b) is not later than \(a))")
            }
        }
    }
}

/// Registries §4's field tables.
public struct MetadataSchema: Sendable {
    public struct Field: Sendable {
        public let baseTag: UInt16
        public let critical: Bool
        public let type: MetadataFieldType
        public let required: Bool
    }

    public let fields: [Field]
    public var crossFieldRules: [CrossFieldRule] = []
    /// Registries §4's frozen per-kind maximum. Derived from `fields`, then pinned against the
    /// contract's own table by `MetadataRegistry.publishedMaxima`.
    public var maximumEncodedLength: Int { 8 + fields.reduce(0) { $0 + 4 + $1.type.maximumWidth } }
}

/// A registered field's wire type and, where the registry states one, its permitted value range.
///
/// The rejection details follow the convention this contract uses everywhere, which registries §4
/// adjudicates: a value outside a **continuous** quantity's bounds is
/// `invalidDescriptor/invalidCombination`, a value that is not a member of an **enumerated** set is
/// `invalidDescriptor/unknownEnum`, and everything §2.2's *encoding* paragraph governs — a wrong
/// registered width, a boolean byte that is neither `0` nor `1`, text that is not clean
/// shortest-form UTF-8 — is `invalidDescriptor/noncanonicalMetadata`, because those are rules about
/// the encoding rather than about the registered value space. Retention (`0…5`) and update state
/// (`1…6`) are enumerations; coordinates, radii and timestamps are not; a boolean is neither.
public enum MetadataFieldType: Sendable {
    /// An enumerated `u8` domain: out-of-range is `unknownEnum`.
    case u8Enum(ClosedRange<UInt8>)
    case u8
    case boolean
    case u16
    case u32(ClosedRange<UInt32>?)
    case u64
    case i32(ClosedRange<Int32>?)
    case i64
    case text(minimum: Int, maximum: Int)
    case bytes(exact: Int)

    var maximumWidth: Int {
        switch self {
        case .u8Enum, .u8, .boolean: return 1
        case .u16: return 2
        case .u32, .i32: return 4
        case .u64, .i64: return 8
        case .text(_, let maximum): return maximum
        case .bytes(let exact): return exact
        }
    }

    /// §2.2: "Schema integers use their exact registered width"; a schema-disallowed width is
    /// `noncanonicalMetadata`.
    func decode(_ value: [UInt8], subject: String) throws -> MetadataValue {
        func requireWidth(_ n: Int) throws {
            guard value.count == n else {
                throw WireFault.noncanonicalMetadata("\(subject): \(value.count) bytes, expected \(n)")
            }
        }
        var reader = ByteReader(value, subject: subject)
        switch self {
        case .u8Enum(let range):
            try requireWidth(1)
            let v = try reader.u8()
            guard range.contains(v) else { throw WireFault.unknownEnum("\(subject): value \(v)") }
            return .u8(v)
        case .u8:
            try requireWidth(1)
            return .u8(try reader.u8())
        case .boolean:
            try requireWidth(1)
            let v = try reader.u8()
            // §2.2 fixes the encoding of a boolean as the byte `0` or `1`, so a third value is a
            // malformed *encoding*, not an unregistered member of an enumeration.
            guard v <= 1 else { throw WireFault.noncanonicalMetadata("\(subject): boolean \(v)") }
            return .boolean(v == 1)
        case .u16: try requireWidth(2); return .u16(try reader.u16())
        case .u32(let range):
            try requireWidth(4)
            let v = try reader.u32()
            if let range, !range.contains(v) {
                throw WireFault.invalidCombination("\(subject): \(v) outside \(range)")
            }
            return .u32(v)
        case .u64: try requireWidth(8); return .u64(try reader.u64())
        case .i32(let range):
            try requireWidth(4)
            let v = try reader.i32()
            if let range, !range.contains(v) {
                throw WireFault.invalidCombination("\(subject): \(v) outside \(range)")
            }
            return .i32(v)
        case .i64: try requireWidth(8); return .i64(try reader.i64())
        case .text(let minimum, let maximum):
            guard value.count >= minimum, value.count <= maximum else {
                throw WireFault.noncanonicalMetadata(
                    "\(subject): text of \(value.count) bytes outside \(minimum)…\(maximum)")
            }
            return .text(try WireText.validate(value, subject: subject))
        case .bytes(let exact):
            try requireWidth(exact)
            return .bytes(value)
        }
    }
}

public enum MetadataRegistry {
    /// Registries §3's durable-request-context ranges. The weather Put envelope declares the
    /// *validated* form of exactly these facts, so the same bounds bind both directions: a bundle
    /// may not declare a coverage centre the request context could never have asked for.
    public static let latitudeE7Range: ClosedRange<Int32> = -900_000_000...900_000_000
    public static let longitudeE7Range: ClosedRange<Int32> = -1_800_000_000...1_800_000_000
    public static let radiusMetresRange: ClosedRange<UInt32> = 1...100_000

    static func schema(kind: ObjectKind, schemaClass: SchemaClass) -> MetadataSchema? {
        switch (kind, schemaClass) {
        // §4.1 Put v1
        case (.route, .put):
            return MetadataSchema(fields: [
                .init(baseTag: 0x0001, critical: true, type: .u8Enum(0...5), required: true)
            ])
        case (.weather, .put):
            return MetadataSchema(
                fields: [
                    .init(baseTag: 0x0001, critical: true, type: .u64, required: true),
                    .init(
                        baseTag: 0x0002, critical: true, type: .i32(latitudeE7Range), required: true),
                    .init(
                        baseTag: 0x0003, critical: true, type: .i32(longitudeE7Range), required: true),
                    .init(
                        baseTag: 0x0004, critical: true, type: .u32(radiusMetresRange), required: true),
                    .init(baseTag: 0x0005, critical: true, type: .i64, required: true),
                    .init(baseTag: 0x0006, critical: true, type: .i64, required: true),
                ],
                crossFieldRules: [
                    // Registries §3: "Must be later than earliest issued UTC."
                    .strictlyLaterI64(
                        earlier: 0x0005, later: 0x0006, what: "valid-until must be later than issued")
                ])
        case (.trip, .put), (.ride, .put), (.volumeManifest, .put), (.updatePackage, .put):
            return MetadataSchema(fields: [])

        // §4.2 SetMetadata v128 — every field individually optional; the empty patch is refused as
        // a *request* (`invalidDescriptor/emptyMetadataPatch`), not as a codec error.
        case (.route, .patch):
            return MetadataSchema(fields: [
                .init(baseTag: 0x0001, critical: true, type: .u8Enum(0...5), required: false),
                .init(baseTag: 0x0002, critical: true, type: .boolean, required: false),
                .init(baseTag: 0x0003, critical: true, type: .text(minimum: 1, maximum: 48), required: false),
            ])
        case (.volumeManifest, .patch):
            return MetadataSchema(fields: [
                .init(baseTag: 0x0001, critical: true, type: .boolean, required: false)
            ])
        case (.trip, .patch), (.ride, .patch), (.weather, .patch), (.updatePackage, .patch):
            return nil  // "Other kinds reject SetMetadata as unsupported."

        // §4.3 Catalog projection v64
        case (.route, .catalogProjection):
            return MetadataSchema(fields: [
                .init(baseTag: 0x0001, critical: true, type: .text(minimum: 1, maximum: 48), required: true),
                .init(baseTag: 0x0002, critical: true, type: .u8Enum(0...5), required: true),
                .init(baseTag: 0x0003, critical: false, type: .boolean, required: false),
                .init(baseTag: 0x0004, critical: false, type: .i64, required: false),
            ])
        case (.trip, .catalogProjection):
            return MetadataSchema(fields: [
                .init(baseTag: 0x0001, critical: true, type: .text(minimum: 1, maximum: 48), required: true),
                .init(baseTag: 0x0002, critical: true, type: .u16, required: true),
            ])
        case (.ride, .catalogProjection):
            return MetadataSchema(fields: [
                .init(baseTag: 0x0001, critical: true, type: .i64, required: true),
                .init(baseTag: 0x0002, critical: true, type: .u32(nil), required: true),
                .init(baseTag: 0x0003, critical: true, type: .u32(nil), required: true),
                .init(baseTag: 0x0004, critical: true, type: .boolean, required: true),
            ])
        case (.weather, .catalogProjection):
            return MetadataSchema(fields: [
                .init(baseTag: 0x0001, critical: true, type: .u64, required: true),
                .init(baseTag: 0x0002, critical: true, type: .i64, required: true),
                .init(baseTag: 0x0003, critical: true, type: .i64, required: true),
            ])
        case (.volumeManifest, .catalogProjection):
            return MetadataSchema(fields: [
                .init(baseTag: 0x0001, critical: true, type: .text(minimum: 1, maximum: 32), required: true),
                .init(baseTag: 0x0002, critical: true, type: .boolean, required: true),
                .init(baseTag: 0x0003, critical: true, type: .u16, required: true),
            ])
        case (.updatePackage, .catalogProjection):
            return MetadataSchema(fields: [
                .init(baseTag: 0x0001, critical: true, type: .text(minimum: 1, maximum: 24), required: true),
                .init(baseTag: 0x0002, critical: true, type: .u8Enum(1...6), required: true),
                .init(baseTag: 0x0003, critical: true, type: .bytes(exact: 32), required: true),
            ])
        }
    }

    /// Registries §4's published maximum-encoded-length table, transcribed so the schemas above can
    /// be proved against it rather than trusted.
    public static let publishedMaxima: [ObjectKind: (put: Int, patch: Int?, catalog: Int)] = [
        .route: (13, 70, 82),
        .trip: (8, nil, 66),
        .ride: (8, nil, 41),
        .weather: (68, nil, 44),
        .volumeManifest: (8, 13, 55),
        .updatePackage: (8, nil, 77),
    ]

    public static func maximumEncodedLength(kind: ObjectKind, schemaClass: SchemaClass) -> Int? {
        schema(kind: kind, schemaClass: schemaClass)?.maximumEncodedLength
    }
}
