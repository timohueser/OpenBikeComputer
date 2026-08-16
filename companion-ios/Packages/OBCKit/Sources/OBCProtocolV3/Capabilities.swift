import Foundation

/// §5's page kind.
public enum CapabilityPageKind: UInt8, Sendable, CaseIterable {
    case resourceLimits = 0
    case subjectCapabilities = 1
}

/// The 12-byte Hello of §5.
public struct Hello: Hashable, Sendable {
    public static let payloadBytes = 12

    public let minimumWireMajor: UInt8
    public let maximumWireMajor: UInt8
    public let clientMaximumControlFrame: UInt16
    public let clientMaximumStreamFrame: UInt16
    /// §5: zero in v3.0.
    public let clientFeatureFlags: UInt32
    public let pageKind: CapabilityPageKind
    public let pageIndex: UInt8

    /// §5.2's negotiation fields: a repeated Hello MUST carry these byte-identically and may differ
    /// only in page kind and index.
    public var negotiationFields: [UInt8] {
        var writer = ByteWriter()
        writer.u8(minimumWireMajor)
        writer.u8(maximumWireMajor)
        writer.u16(clientMaximumControlFrame)
        writer.u16(clientMaximumStreamFrame)
        writer.u32(clientFeatureFlags)
        return writer.bytes
    }

    public static func decode(_ bytes: [UInt8]) throws -> Hello {
        try requireExactPayload(bytes.count, payloadBytes, "Hello")
        var reader = ByteReader(bytes, subject: "Hello")
        let minimum = try reader.u8()
        let maximum = try reader.u8()
        let control = try reader.u16()
        let stream = try reader.u16()
        let features = try reader.u32()
        let pageKindRaw = try reader.u8()
        let pageIndex = try reader.u8()

        guard minimum != 0, minimum <= maximum else {
            throw WireFault.invalidCombination("Hello: major range \(minimum)…\(maximum)")
        }
        guard control <= WireLimits.maximumControlFrame else {
            throw WireFault.frameBounds("Hello: client control maximum \(control)")
        }
        guard stream <= WireLimits.maximumStreamFrame else {
            throw WireFault.frameBounds("Hello: client stream maximum \(stream)")
        }
        guard features == 0 else {
            throw WireFault.unsupportedFlags("Hello: client feature flags \(features)")
        }
        guard let pageKind = CapabilityPageKind(rawValue: pageKindRaw) else {
            throw WireFault.unknownEnum("Hello: page kind \(pageKindRaw)")
        }
        // §5: "A nonzero resource-page index … is invalidDescriptor."
        guard pageKind != .resourceLimits || pageIndex == 0 else {
            throw WireFault.invalidCombination("Hello: resource page index \(pageIndex)")
        }
        return Hello(
            minimumWireMajor: minimum, maximumWireMajor: maximum,
            clientMaximumControlFrame: control, clientMaximumStreamFrame: stream,
            clientFeatureFlags: features, pageKind: pageKind, pageIndex: pageIndex)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.u8(minimumWireMajor)
        writer.u8(maximumWireMajor)
        writer.u16(clientMaximumControlFrame)
        writer.u16(clientMaximumStreamFrame)
        writer.u32(clientFeatureFlags)
        writer.u8(pageKind.rawValue)
        writer.u8(pageIndex)
        return writer.bytes
    }
}

/// §5's status flags.
public struct CapabilityStatusFlags: OptionSet, Sendable, Hashable {
    public let rawValue: UInt16
    public init(rawValue: UInt16) { self.rawValue = rawValue }
    public static let storeAvailable = CapabilityStatusFlags(rawValue: 1 << 0)
    public static let authenticated = CapabilityStatusFlags(rawValue: 1 << 1)
    public static let heavyTransferBusy = CapabilityStatusFlags(rawValue: 1 << 2)
    public static let developerUnlocked = CapabilityStatusFlags(rawValue: 1 << 3)
    static let defined: UInt16 = 0x000F
}

/// §5's command flags, bits 0…16. "A request for an operation whose bit is clear is
/// `unsupportedCapability/opcode`."
public struct CommandFlags: OptionSet, Sendable, Hashable {
    public let rawValue: UInt32
    public init(rawValue: UInt32) { self.rawValue = rawValue }
    public static let queryOperation = CommandFlags(rawValue: 1 << 0)
    public static let queryCatalog = CommandFlags(rawValue: 1 << 1)
    public static let queryDraft = CommandFlags(rawValue: 1 << 2)
    public static let queryWeatherRequest = CommandFlags(rawValue: 1 << 3)
    public static let beginDraft = CommandFlags(rawValue: 1 << 4)
    public static let startDraftPart = CommandFlags(rawValue: 1 << 5)
    public static let finalizeDraft = CommandFlags(rawValue: 1 << 6)
    public static let abortOperation = CommandFlags(rawValue: 1 << 7)
    public static let installUpdate = CommandFlags(rawValue: 1 << 8)
    public static let acknowledgeRideImported = CommandFlags(rawValue: 1 << 9)
    public static let getDeviceStatus = CommandFlags(rawValue: 1 << 10)
    public static let getConfig = CommandFlags(rawValue: 1 << 11)
    public static let setConfig = CommandFlags(rawValue: 1 << 12)
    public static let setClock = CommandFlags(rawValue: 1 << 13)
    public static let forgetBond = CommandFlags(rawValue: 1 << 14)
    public static let echo = CommandFlags(rawValue: 1 << 15)
    public static let resetStore = CommandFlags(rawValue: 1 << 16)
    static let defined: UInt32 = 0x0001_FFFF
}

/// §5's 20-byte subject entry.
public struct SubjectEntry: Hashable, Sendable {
    public static let entryBytes = 20

    public let namespace: SubjectNamespace
    public let kindCode: UInt16
    public let operationFlags: SubjectOperationFlags
    public let policyFlags: SubjectPolicyFlags
    public let putSchemaVersion: UInt8
    public let patchSchemaVersion: UInt8
    public let catalogSchemaVersion: UInt8
    public let maximumLength: UInt64

    public var objectKind: ObjectKind? {
        namespace == .logicalObjectKind ? ObjectKind(rawValue: kindCode) : nil
    }
    public var draftPartKind: DraftPartKind? {
        namespace == .draftPartKind ? DraftPartKind(rawValue: kindCode) : nil
    }

    public static func decode(_ bytes: [UInt8]) throws -> SubjectEntry {
        try requireExactPayload(bytes.count, entryBytes, "subject entry")
        var reader = ByteReader(bytes, subject: "subject entry")
        let namespaceRaw = try reader.u8()
        guard let namespace = SubjectNamespace(rawValue: namespaceRaw) else {
            throw WireFault.unknownEnum("subject entry: namespace \(namespaceRaw)")
        }
        try reader.reserved(1, "subject entry offset 1")
        let kindCode = try reader.u16()
        let operationRaw = try reader.u16()
        let policyRaw = try reader.u16()
        let putVersion = try reader.u8()
        let patchVersion = try reader.u8()
        let catalogVersion = try reader.u8()
        try reader.reserved(1, "subject entry offset 11")
        let maximumLength = try reader.u64()

        guard operationRaw & ~SubjectOperationFlags.defined == 0 else {
            throw WireFault.reservedBits("subject entry: operation flags \(operationRaw)")
        }
        guard policyRaw & ~SubjectPolicyFlags.defined == 0 else {
            throw WireFault.reservedBits("subject entry: policy flags \(policyRaw)")
        }
        let operationFlags = SubjectOperationFlags(rawValue: operationRaw)

        switch namespace {
        case .logicalObjectKind:
            guard let kind = ObjectKind(rawValue: kindCode) else {
                throw WireFault.unknownEnum("subject entry: ObjectKind \(kindCode)")
            }
            // Registries §1: "a device advertises a subset of the permitted set, never a superset";
            // "a `no` is normative".
            guard operationFlags.isSubset(of: kind.permittedOperationFlags) else {
                throw WireFault.invalidCombination(
                    "subject entry: \(kind.name) advertises an operation its lifecycle forbids")
            }
            // §5: the patch schema version "takes exactly two legal values" — 128 with the
            // set-metadata flag set, zero with it clear. "Any other value, in either direction, is
            // invalidDescriptor."
            let expectedPatch: UInt8 = operationFlags.contains(.setMetadata) ? 128 : 0
            guard patchVersion == expectedPatch else {
                throw WireFault.invalidCombination(
                    "subject entry: patch schema version \(patchVersion), expected \(expectedPatch)")
            }
            // Registries §4: a device advertises the registry constant, or zero for an operation it
            // does not support.
            guard putVersion == 0 || putVersion == SchemaClass.put.version else {
                throw WireFault.invalidCombination("subject entry: put schema version \(putVersion)")
            }
            guard catalogVersion == 0 || catalogVersion == SchemaClass.catalogProjection.version
            else {
                throw WireFault.invalidCombination(
                    "subject entry: catalog schema version \(catalogVersion)")
            }
        case .draftPartKind:
            guard DraftPartKind(rawValue: kindCode) != nil else {
                throw WireFault.unknownEnum("subject entry: DraftPartKind \(kindCode)")
            }
            // §5: "Draft-part subjects advertise put and optional resumable upload only; all three
            // schema versions are zero."
            let permitted: SubjectOperationFlags = [.put, .resumableUpload]
            guard operationFlags.isSubset(of: permitted) else {
                throw WireFault.invalidCombination(
                    "subject entry: draft part advertises more than put + resumable upload")
            }
            guard putVersion == 0, patchVersion == 0, catalogVersion == 0 else {
                throw WireFault.invalidCombination("subject entry: draft part schema version nonzero")
            }
        }

        return SubjectEntry(
            namespace: namespace, kindCode: kindCode, operationFlags: operationFlags,
            policyFlags: SubjectPolicyFlags(rawValue: policyRaw), putSchemaVersion: putVersion,
            patchSchemaVersion: patchVersion, catalogSchemaVersion: catalogVersion,
            maximumLength: maximumLength)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.u8(namespace.rawValue)
        writer.u8(0)
        writer.u16(kindCode)
        writer.u16(operationFlags.rawValue)
        writer.u16(policyFlags.rawValue)
        writer.u8(putSchemaVersion)
        writer.u8(patchSchemaVersion)
        writer.u8(catalogSchemaVersion)
        writer.u8(0)
        writer.u64(maximumLength)
        return writer.bytes
    }
}

/// §5.1's 56-byte ResourceLimits block. The fixed values it reports are mirrors of the storage
/// contract, so this decoder validates the block's *shape* and the two facts §5.1 makes normative
/// here (its codec version and its reserved regions), not every capacity number.
public struct ResourceLimits: Hashable, Sendable {
    public static let blockBytes = 56

    public let codecVersion: UInt8
    public let logicalCatalogHeads: UInt16
    public let normalActiveClaimedOperations: UInt8
    public let resumableWorkSlots: UInt8
    public let activeDraftParents: UInt8
    public let draftPartsOfActiveParent: UInt8
    public let childrenPerManifest: UInt8
    public let mountedMapDataFiles: UInt8
    public let liveReaderLeases: UInt8
    public let retainedPreviousGenerations: UInt8
    public let retainedTerminalResults: UInt16
    public let inactiveWorkHorizon: UInt16
    public let maximumSingleGenerationLength: UInt64
    public let currentlyAvailableReservationBytes: UInt64
    public let routeCatalogHeads: UInt16
    public let tripCatalogHeads: UInt16
    public let rideCatalogHeads: UInt16
    public let weatherCatalogHeads: UInt16
    public let volumeManifestCatalogHeads: UInt16
    public let updatePackageCatalogHeads: UInt16
    public let attachedHeavyStreamSessions: UInt8
    public let reservedMaintenanceClaims: UInt8
    public let activeOrRecoverableRideSlots: UInt8

    static func decode(_ reader: inout ByteReader) throws -> ResourceLimits {
        let codecVersion = try reader.u8()
        let blockLength = try reader.u8()
        guard blockLength == UInt8(blockBytes) else {
            throw WireFault.invalidCombination("ResourceLimits: block length \(blockLength)")
        }
        let flags = try reader.u16()
        guard flags == 0 else { throw WireFault.reservedBits("ResourceLimits: flags") }
        let heads = try reader.u16()
        let claims = try reader.u8()
        let work = try reader.u8()
        let parents = try reader.u8()
        let parts = try reader.u8()
        let children = try reader.u8()
        let maps = try reader.u8()
        let leases = try reader.u8()
        let retainedGenerations = try reader.u8()
        let retainedResults = try reader.u16()
        let horizon = try reader.u16()
        // §5.1: journal capacity was formerly reported at byte 18; the field is reserved and zero.
        try reader.reserved(2, "ResourceLimits byte 18")
        let maximumGeneration = try reader.u64()
        let available = try reader.u64()
        let route = try reader.u16()
        let trip = try reader.u16()
        let ride = try reader.u16()
        let weather = try reader.u16()
        let volume = try reader.u16()
        let update = try reader.u16()
        let heavy = try reader.u8()
        let maintenance = try reader.u8()
        let rideSlots = try reader.u8()
        try reader.reserved(5, "ResourceLimits byte 51")

        guard retainedResults == UInt16(WireLimits.retainedResults) else {
            throw WireFault.invalidCombination("ResourceLimits: retained results \(retainedResults)")
        }
        return ResourceLimits(
            codecVersion: codecVersion, logicalCatalogHeads: heads,
            normalActiveClaimedOperations: claims, resumableWorkSlots: work,
            activeDraftParents: parents, draftPartsOfActiveParent: parts,
            childrenPerManifest: children, mountedMapDataFiles: maps, liveReaderLeases: leases,
            retainedPreviousGenerations: retainedGenerations, retainedTerminalResults: retainedResults,
            inactiveWorkHorizon: horizon, maximumSingleGenerationLength: maximumGeneration,
            currentlyAvailableReservationBytes: available, routeCatalogHeads: route,
            tripCatalogHeads: trip, rideCatalogHeads: ride, weatherCatalogHeads: weather,
            volumeManifestCatalogHeads: volume, updatePackageCatalogHeads: update,
            attachedHeavyStreamSessions: heavy, reservedMaintenanceClaims: maintenance,
            activeOrRecoverableRideSlots: rideSlots)
    }

    func encode(into writer: inout ByteWriter) {
        writer.u8(codecVersion)
        writer.u8(UInt8(Self.blockBytes))
        writer.u16(0)
        writer.u16(logicalCatalogHeads)
        writer.u8(normalActiveClaimedOperations)
        writer.u8(resumableWorkSlots)
        writer.u8(activeDraftParents)
        writer.u8(draftPartsOfActiveParent)
        writer.u8(childrenPerManifest)
        writer.u8(mountedMapDataFiles)
        writer.u8(liveReaderLeases)
        writer.u8(retainedPreviousGenerations)
        writer.u16(retainedTerminalResults)
        writer.u16(inactiveWorkHorizon)
        writer.zeros(2)
        writer.u64(maximumSingleGenerationLength)
        writer.u64(currentlyAvailableReservationBytes)
        writer.u16(routeCatalogHeads)
        writer.u16(tripCatalogHeads)
        writer.u16(rideCatalogHeads)
        writer.u16(weatherCatalogHeads)
        writer.u16(volumeManifestCatalogHeads)
        writer.u16(updatePackageCatalogHeads)
        writer.u8(attachedHeavyStreamSessions)
        writer.u8(reservedMaintenanceClaims)
        writer.u8(activeOrRecoverableRideSlots)
        writer.zeros(5)
    }
}

/// One Capabilities page: the 56-byte common prefix of §5 plus either the ResourceLimits block or
/// up to two complete subject entries.
public struct CapabilitiesPage: Hashable, Sendable {
    public static let prefixBytes = 56

    public let selectedWireMajor: UInt8
    public let storageFormatVersion: UInt8
    public let statusFlags: CapabilityStatusFlags
    public let storeId: StoreId
    public let negotiatedMaximumControlFrame: UInt16
    public let negotiatedMaximumStreamFrame: UInt16
    public let checkpointGranule: UInt32
    public let retainedResultCapacity: UInt16
    public let metadataEnvelopeLimit: UInt16
    public let catalogMetadataLimit: UInt16
    public let protocolMinimumControlFrame: UInt16
    public let protocolMinimumStreamFrame: UInt16
    public let linkKind: LinkKind
    public let authenticated: Bool
    public let capabilityRevision: UInt32
    public let commandFlags: CommandFlags
    public let totalSubjectCount: UInt16
    public let returnedPageKind: CapabilityPageKind
    public let returnedPageIndex: UInt8
    public let totalPagesOfThisKind: UInt8
    /// Byte 54; on a resource page it MUST equal the block's own byte 0.
    public let resourceLimitsCodecVersion: UInt8
    /// Byte 55; §5 makes this the only place a wire minor is learnable.
    public let deviceWireMinor: UInt8
    public let resourceLimits: ResourceLimits?
    public let subjects: [SubjectEntry]

    public static func decode(_ bytes: [UInt8]) throws -> CapabilitiesPage {
        guard bytes.count >= prefixBytes else {
            throw WireFault.truncated("Capabilities: \(bytes.count) bytes")
        }
        var reader = ByteReader(bytes, subject: "Capabilities")
        let selectedMajor = try reader.u8()
        let storageFormat = try reader.u8()
        let statusRaw = try reader.u16()
        let storeId = StoreId(unchecked: try reader.opaque16())
        let control = try reader.u16()
        let stream = try reader.u16()
        let granule = try reader.u32()
        let retained = try reader.u16()
        let metadataLimit = try reader.u16()
        let catalogLimit = try reader.u16()
        let minControl = try reader.u16()
        let minStream = try reader.u16()
        let linkRaw = try reader.u8()
        let authRaw = try reader.u8()
        let capabilityRevision = try reader.u32()
        let commandRaw = try reader.u32()
        let totalSubjects = try reader.u16()
        let pageKindRaw = try reader.u8()
        let pageIndex = try reader.u8()
        let returnedSubjects = try reader.u8()
        let totalPages = try reader.u8()
        let resourceCodecVersion = try reader.u8()
        let deviceMinor = try reader.u8()

        guard selectedMajor == WireLimits.major else {
            throw WireFault.unsupportedMajor("Capabilities: selected major \(selectedMajor)")
        }
        guard storageFormat == 1 else {
            throw WireFault.invalidCombination("Capabilities: storage format \(storageFormat)")
        }
        guard statusRaw & ~CapabilityStatusFlags.defined == 0 else {
            throw WireFault.unsupportedFlags("Capabilities: status flags \(statusRaw)")
        }
        let status = CapabilityStatusFlags(rawValue: statusRaw)
        // §5: "StoreId, zero only when store-available is clear."
        if status.contains(.storeAvailable) {
            guard !storeId.isZero else {
                throw WireFault.invalidCombination("Capabilities: store available with a zero StoreId")
            }
        } else {
            guard storeId.isZero else {
                throw WireFault.reservedBits("Capabilities: StoreId set with store-available clear")
            }
        }
        guard control >= WireLimits.minimumControlFrame, control <= WireLimits.maximumControlFrame
        else {
            throw WireFault.invalidCombination("Capabilities: negotiated control frame \(control)")
        }
        guard stream >= WireLimits.minimumStreamFrame, stream <= WireLimits.maximumStreamFrame else {
            throw WireFault.invalidCombination("Capabilities: negotiated stream frame \(stream)")
        }
        guard retained == UInt16(WireLimits.retainedResults),
            metadataLimit == UInt16(WireLimits.metadataEnvelopeCeiling),
            catalogLimit == UInt16(WireLimits.catalogMetadataCeiling),
            minControl == UInt16(WireLimits.minimumControlFrame),
            minStream == UInt16(WireLimits.minimumStreamFrame)
        else {
            throw WireFault.invalidCombination("Capabilities: a frozen limit disagrees with §1")
        }
        guard let linkKind = LinkKind(rawValue: linkRaw) else {
            throw WireFault.unknownEnum("Capabilities: link kind \(linkRaw)")
        }
        guard authRaw <= 1 else { throw WireFault.unknownEnum("Capabilities: auth state \(authRaw)") }
        guard status.contains(.authenticated) == (authRaw == 1) else {
            throw WireFault.invalidCombination("Capabilities: auth state disagrees with the status bit")
        }
        guard commandRaw & ~CommandFlags.defined == 0 else {
            throw WireFault.unsupportedFlags("Capabilities: command flags \(commandRaw)")
        }
        guard totalSubjects <= UInt16(WireLimits.subjectCeiling) else {
            throw WireFault.invalidCombination("Capabilities: \(totalSubjects) subjects")
        }
        guard let pageKind = CapabilityPageKind(rawValue: pageKindRaw) else {
            throw WireFault.unknownEnum("Capabilities: returned page kind \(pageKindRaw)")
        }
        guard deviceMinor == WireLimits.minor else {
            throw WireFault.unsupportedMinor("Capabilities: device wire minor \(deviceMinor)")
        }

        var resourceLimits: ResourceLimits?
        var subjects: [SubjectEntry] = []
        switch pageKind {
        case .resourceLimits:
            guard pageIndex == 0, returnedSubjects == 0, totalPages == 1 else {
                throw WireFault.invalidCombination("Capabilities: malformed resource page paging")
            }
            guard reader.remaining >= ResourceLimits.blockBytes else {
                throw WireFault.truncated("Capabilities: resource block")
            }
            // §5: "A server MUST emit equal values; a client that observes a mismatch MUST reject
            // that page and abandon discovery rather than decode either block."
            guard reader.bytes[reader.index] == resourceCodecVersion else {
                throw WireFault.invalidCombination(
                    "Capabilities: byte 54 \(resourceCodecVersion) != block byte 0 \(reader.bytes[reader.index])")
            }
            resourceLimits = try ResourceLimits.decode(&reader)
        case .subjectCapabilities:
            let expectedPages = (Int(totalSubjects) + 1) / 2
            guard Int(totalPages) == expectedPages else {
                throw WireFault.invalidCombination(
                    "Capabilities: total pages \(totalPages), expected \(expectedPages)")
            }
            // §5: a zero-subject device answers page zero; "only a subject page index above zero is
            // invalidDescriptor in that case."
            let firstSubject = Int(pageIndex) * 2
            guard firstSubject < Int(totalSubjects) || (totalSubjects == 0 && pageIndex == 0) else {
                throw WireFault.invalidCombination("Capabilities: subject page index \(pageIndex)")
            }
            let expectedSubjects = min(2, Int(totalSubjects) - firstSubject)
            guard Int(returnedSubjects) == max(0, expectedSubjects) else {
                throw WireFault.invalidCombination(
                    "Capabilities: returned subject count \(returnedSubjects)")
            }
            for _ in 0..<Int(returnedSubjects) {
                subjects.append(try SubjectEntry.decode(Array(try reader.take(SubjectEntry.entryBytes))))
            }
            // §5: entries are returned in ascending `(namespace, kind_code)` order.
            for (previous, next) in zip(subjects, subjects.dropFirst()) {
                let a = (previous.namespace.rawValue, previous.kindCode)
                let b = (next.namespace.rawValue, next.kindCode)
                guard a < b else {
                    throw WireFault.invalidCombination("Capabilities: subjects out of order")
                }
            }
        }
        try reader.requireExhausted("the capability page")

        return CapabilitiesPage(
            selectedWireMajor: selectedMajor, storageFormatVersion: storageFormat,
            statusFlags: status, storeId: storeId, negotiatedMaximumControlFrame: control,
            negotiatedMaximumStreamFrame: stream, checkpointGranule: granule,
            retainedResultCapacity: retained, metadataEnvelopeLimit: metadataLimit,
            catalogMetadataLimit: catalogLimit, protocolMinimumControlFrame: minControl,
            protocolMinimumStreamFrame: minStream, linkKind: linkKind, authenticated: authRaw == 1,
            capabilityRevision: capabilityRevision, commandFlags: CommandFlags(rawValue: commandRaw),
            totalSubjectCount: totalSubjects, returnedPageKind: pageKind,
            returnedPageIndex: pageIndex, totalPagesOfThisKind: totalPages,
            resourceLimitsCodecVersion: resourceCodecVersion, deviceWireMinor: deviceMinor,
            resourceLimits: resourceLimits, subjects: subjects)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.u8(selectedWireMajor)
        writer.u8(storageFormatVersion)
        writer.u16(statusFlags.rawValue)
        writer.raw(storeId.bytes)
        writer.u16(negotiatedMaximumControlFrame)
        writer.u16(negotiatedMaximumStreamFrame)
        writer.u32(checkpointGranule)
        writer.u16(retainedResultCapacity)
        writer.u16(metadataEnvelopeLimit)
        writer.u16(catalogMetadataLimit)
        writer.u16(protocolMinimumControlFrame)
        writer.u16(protocolMinimumStreamFrame)
        writer.u8(linkKind.rawValue)
        writer.u8(authenticated ? 1 : 0)
        writer.u32(capabilityRevision)
        writer.u32(commandFlags.rawValue)
        writer.u16(totalSubjectCount)
        writer.u8(returnedPageKind.rawValue)
        writer.u8(returnedPageIndex)
        writer.u8(try narrowU8(subjects.count, "Capabilities: returned subject count"))
        writer.u8(totalPagesOfThisKind)
        writer.u8(resourceLimitsCodecVersion)
        writer.u8(deviceWireMinor)
        resourceLimits?.encode(into: &writer)
        for subject in subjects { writer.raw(try subject.encoded()) }
        return writer.bytes
    }
}

/// A short fixed-size payload is `invalidFrame/truncated`; a long one is `invalidFrame/trailingBytes`.
func requireExactPayload(_ actual: Int, _ expected: Int, _ subject: String) throws {
    if actual < expected {
        throw WireFault.truncated("\(subject): \(actual) bytes, expected \(expected)")
    }
    if actual > expected {
        throw WireFault.trailingBytes("\(subject): \(actual) bytes, expected \(expected)")
    }
}
