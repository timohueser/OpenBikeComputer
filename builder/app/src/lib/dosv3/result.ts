/**
 * The error taxonomy of Device_Object_Protocol_v3.md §12, and the total-decoding result type
 * every entry point in this module returns.
 *
 * Two things live here and they are deliberately the same vocabulary:
 *
 * 1. the categories and category-scoped detail codes a *device* puts in an ErrorBody, and
 * 2. the reason a *decoder* rejected some bytes.
 *
 * §12's table is the taxonomy for both, which is what lets the negative fixtures state one expected
 * `category`/`detail` pair and have it mean the same thing on the wire and in a codec. Nothing here
 * throws: a malformed frame is a value, because a client that crashes on a bad frame has no way to
 * report the failure the frame was trying to describe.
 */

export const CATEGORY = {
    incompatibleVersion: 1,
    unsupportedCapability: 2,
    authenticationFailed: 3,
    authorizationFailed: 4,
    busy: 5,
    invalidFrame: 6,
    invalidDescriptor: 7,
    invalidOffset: 8,
    invalidSession: 9,
    objectNotFound: 10,
    revisionConflict: 11,
    insufficientSpace: 12,
    checksumFailure: 13,
    semanticValidation: 14,
    mediaUnavailable: 15,
    mediaIo: 16,
    cancelled: 17,
    linkLost: 18,
    operationIdConflict: 19,
    resourceLimit: 20,
    catalogChanged: 21,
    internal: 22,
} as const;

export type CategoryName = keyof typeof CATEGORY;

export const CATEGORY_NAME: ReadonlyMap<number, CategoryName> = new Map(
    (Object.entries(CATEGORY) as [CategoryName, number][]).map(([name, value]) => [value, name]),
);

/**
 * §12's complete detail table, category-scoped. "Detail codes are category-scoped; the same number
 * in another category has no relationship", so this is a map of maps and never a flat enum.
 *
 * Reserved-and-never-emitted details (`busy/draftParts`, `objectNotFound/requestedRevision`, …) are
 * listed: their numbers stay burned, and the suite carries decode-only rows for them that a v3.0
 * decoder must still name rather than report as unknown.
 */
export const DETAILS: Readonly<Record<CategoryName, Readonly<Record<string, number>>>> = {
    incompatibleVersion: { unsupportedMajor: 1, unsupportedMinor: 2 },
    unsupportedCapability: {
        opcode: 1,
        logicalKind: 2,
        draftPartKind: 3,
        feature: 4,
        schemaVersion: 5,
        nonCancellableOperation: 6,
    },
    authenticationFailed: { missingCredential: 1, invalidCredential: 2, expiredCredential: 3 },
    authorizationFailed: {
        principalScope: 1,
        operationOwner: 2,
        domainRead: 3,
        domainWrite: 4,
        installAuthority: 5,
        deviceControl: 6,
    },
    busy: {
        heavyTransfer: 1,
        normalOperationClaims: 2,
        uploadWorkSlots: 3,
        draftParents: 4,
        draftParts: 5,
        readerLeases: 6,
        maintenanceCancellationRecoveryClaim: 7,
        maintenance: 8,
        rideSlot: 9,
        retainedPrevious: 10,
    },
    invalidFrame: {
        malformedHeader: 1,
        recordLength: 2,
        magic: 3,
        payloadLength: 4,
        frameBounds: 5,
        truncated: 6,
        trailingBytes: 7,
    },
    invalidDescriptor: {
        reservedBits: 1,
        unknownEnum: 2,
        invalidCombination: 3,
        nestedLength: 4,
        noncanonicalMetadata: 5,
        duplicateField: 6,
        outOfOrderField: 7,
        unsupportedFlags: 8,
        zeroRequestId: 9,
        emptyMetadataPatch: 10,
    },
    invalidOffset: { unexpectedOffset: 1, checkpointBoundary: 2 },
    invalidSession: { unknown: 1, staleConnection: 2, wrongPrincipal: 3, wrongLink: 4, wrongDirection: 5 },
    objectNotFound: {
        logicalObject: 1,
        requestedRevision: 2,
        draftParentUnknown: 3,
        operationTerminal: 4,
        resumableWork: 5,
        weatherRequestContext: 6,
    },
    revisionConflict: { object: 1, repository: 2, singleton: 3 },
    insufficientSpace: { reservationBytes: 1, catalogCapacity: 2, retainedPrevious: 3 },
    checksumFailure: { wholePayload: 1, durablePrefix: 2, cursor: 3 },
    // With namespace 0 the only registered semantic detail is the device-control plane's
    // clockRegression; with an ObjectKind namespace the domain registry owns the table
    // (see SEMANTIC_DETAILS in registry.ts).
    semanticValidation: { clockRegression: 1 },
    mediaUnavailable: { noCard: 1, unmounted: 2, recoveryReadOnly: 3 },
    mediaIo: { read: 1, write: 2, synchronize: 3, uncertainCommit: 4 },
    cancelled: { clientCancelled: 1, superseded: 2, userRequested: 3, workExpired: 4 },
    linkLost: { control: 1, stream: 2 },
    operationIdConflict: { intentDigest: 1 },
    resourceLimit: {
        minimumControlFrame: 1,
        minimumStreamFrame: 2,
        objectLength: 3,
        normalOperationClaims: 4,
        uploadWorkSlots: 5,
        draftParents: 6,
        draftParts: 7,
        manifestChildren: 8,
        readerLeases: 9,
        catalogHeads: 10,
        mountedFiles: 11,
        rideSlot: 12,
    },
    catalogChanged: { catalogSnapshot: 1, draftSnapshot: 2, capabilitySnapshot: 3 },
    internal: { invariant: 1, codec: 2, recoveryReconciliation: 3 },
};

const DETAIL_NAMES: ReadonlyMap<CategoryName, ReadonlyMap<number, string>> = new Map(
    (Object.keys(DETAILS) as CategoryName[]).map((category) => [
        category,
        new Map(Object.entries(DETAILS[category]).map(([name, value]) => [value, name])),
    ]),
);

/**
 * The name a category/detail pair carries. Detail zero means "no narrower fact"; an unregistered
 * detail is preserved for forward diagnostics rather than rejected, exactly as §12 requires.
 */
export function detailName(category: CategoryName, detail: number): string {
    if (detail === 0) return "none";
    return DETAIL_NAMES.get(category)?.get(detail) ?? "unknown";
}

/** Retry guidance values, §12. */
export const GUIDANCE = {
    rejectPermanently: 0,
    retrySameRequest: 1,
    retryAfterSuppliedDelay: 2,
    retryAfterOwnerRelease: 3,
    reconnectThenQueryOperation: 4,
    queryOperationNow: 5,
    resumeAtExpectedOffset: 6,
    refreshCatalogState: 7,
    newOperationIdForNewIntent: 8,
    retryAfterUserAction: 9,
} as const;
export const MAX_GUIDANCE = 9;

/** ErrorBody owner byte, §12. Values 1–3 deliberately agree with the §5 link-kind byte. */
export const OWNER = { none: 0, ble: 1, usb: 2, test: 3, localProducer: 4, maintenance: 5 } as const;
export const MAX_OWNER = 5;

/** A typed failure. `category`/`detail` are the §12 taxonomy; `message` is for humans only. */
export interface DosError {
    readonly category: CategoryName;
    readonly categoryValue: number;
    readonly detail: string;
    readonly detailValue: number;
    readonly message: string;
    /** Set on a semanticValidation failure that names an ObjectKind namespace. */
    readonly namespace?: number;
}

export type DosResult<T> = { readonly ok: true; readonly value: T } | { readonly ok: false; readonly error: DosError };

export const ok = <T>(value: T): DosResult<T> => ({ ok: true, value });

export function fail<T = never>(category: CategoryName, detail: string, message: string): DosResult<T> {
    return { ok: false, error: dosError(category, detail, message) };
}

export function dosError(category: CategoryName, detail: string, message: string): DosError {
    const detailValue = detail === "none" ? 0 : (DETAILS[category] as Record<string, number>)[detail];
    if (detailValue === undefined) throw new Error(`${detail} is not a registered detail of ${category}`);
    return { category, categoryValue: CATEGORY[category], detail, detailValue, message };
}

/**
 * The internal control-flow carrier. Decoders are written as straight-line readers and signal a
 * rejection by throwing this; every public entry point catches it and returns the failure as a
 * value. Nothing else is ever thrown out of this module for malformed input.
 */
export class DecodeFault extends Error {
    readonly dos: DosError;

    constructor(error: DosError) {
        super(`${error.category}/${error.detail}: ${error.message}`);
        this.name = "DecodeFault";
        this.dos = error;
    }
}

export function reject(category: CategoryName, detail: string, message: string): never {
    throw new DecodeFault(dosError(category, detail, message));
}

/** Runs a total decoder: a DecodeFault becomes an error value, anything else is a real bug. */
export function decoding<T>(run: () => T): DosResult<T> {
    try {
        return ok(run());
    } catch (cause) {
        if (cause instanceof DecodeFault) return { ok: false, error: cause.dos };
        throw cause;
    }
}

/** Unwraps a result, throwing on failure. For call sites that have already proven success. */
export function unwrap<T>(result: DosResult<T>): T {
    if (!result.ok) throw new Error(`${result.error.category}/${result.error.detail}: ${result.error.message}`);
    return result.value;
}
