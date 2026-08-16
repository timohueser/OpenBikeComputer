import Foundation
import OBCProtocolV3

/// One place that knows how to run each fixture kind. Every suite in this target funnels through
/// it, so "exercised" means exactly the same thing everywhere — which is what lets the drift guard
/// prove coverage by walking `manifest.json` alone.
enum VectorExerciser {
    /// The StoreId the whole suite is written against; needed for the ResetStore echo predicate,
    /// which is a device-side check over the request *and* the device's own reported state.
    /// Built through the public failable initializer, like any application caller would: the
    /// unchecked one is internal, and this file deliberately imports the module without
    /// `@testable` so the suite is exercised across the real public surface.
    static let suiteStoreId: StoreId = {
        guard let bytes = try? "3c92000099164ebaabc2342fe08f6b10".hexBytes,
            let storeId = StoreId(bytes: bytes)
        else { fatalError("the suite StoreId literal is malformed") }
        return storeId
    }()

    static func exercise(_ entry: DeviceObjectVectors.Entry) throws {
        let json = try DeviceObjectVectors.json(entry)
        // The manifest's own SHA-256 is part of the contract: an unreviewed fixture rewrite has to
        // fail, not silently redefine the expectation.
        let actual = DeviceObjectVectors.sha256Hex(try DeviceObjectVectors.rawBytes(entry))
        guard actual == entry.sha256 else {
            throw VectorError("\(entry): sha256 \(actual) != manifest \(entry.sha256)")
        }
        guard json["suite"] as? String == "device-object-v2" else {
            throw VectorError("\(entry): wrong suite")
        }
        switch json["kind"] as? String {
        case "control": try exerciseControl(entry, json)
        case "canonicalIntent": try exerciseCanonicalIntent(entry, json)
        case "frameLimitDerivation": try exerciseFrameLimits(entry, json)
        case "negative": try exerciseNegative(entry, json)
        case "stream": try exerciseStream(entry, json)
        case "transcript": try exerciseTranscript(entry, json)
        case let other: throw VectorError("\(entry): unhandled fixture kind \(other ?? "nil")")
        }
        ExerciseLog.shared.record(entry.name)
    }

    // MARK: control

    static func exerciseControl(_ entry: DeviceObjectVectors.Entry, _ json: [String: Any]) throws {
        let frameBytes = try (json["frame"] as? String ?? "").hexBytes
        let payloadBytes = try (json["payload"] as? String ?? "").hexBytes
        guard let header = json["header"] as? [String: Any] else {
            throw VectorError("\(entry): no header")
        }
        let frame = try ControlFrame.decode(frameBytes)

        // Header facts, from the fixture rather than from the decoder's own output.
        guard header["magic"] as? String == "OBCP" else { throw VectorError("\(entry): magic") }
        guard header["major"] as? Int == 3, header["minor"] as? Int == 0 else {
            throw VectorError("\(entry): version")
        }
        guard Int(frame.opcode.rawValue) == (json["opcode"] as? [String: Any])?["value"] as? Int
        else { throw VectorError("\(entry): opcode") }
        guard Int(frame.flags.rawValue) == header["flags"] as? Int else {
            throw VectorError("\(entry): flags")
        }
        guard Int(frame.requestId.rawValue) == header["requestId"] as? Int else {
            throw VectorError("\(entry): RequestId")
        }
        guard payloadBytes.count == header["payloadLength"] as? Int else {
            throw VectorError("\(entry): payload length disagrees with the header")
        }
        guard (json["direction"] as? String == "response") == frame.isResponse else {
            throw VectorError("\(entry): direction")
        }

        // Byte-exact re-encoding, payload and whole frame.
        guard try frame.encoded() == frameBytes else {
            throw VectorError("\(entry): re-encoded frame differs\n  \(try frame.encoded().hexString)")
        }
    }

    // MARK: canonical intent

    /// The intent goldens are judged against an intent rebuilt from the *request* fixture that
    /// carries the same semantic body, so a digest produced by this codec's own encoder is never
    /// the evidence.
    static let intentSources: [String: String] = [
        "intent-start-upload-create-route": "controls/start-upload-create-route.json",
        "intent-start-upload-replace-route": "controls/start-upload-replace-route-at-revision.json",
        "intent-begin-draft": "controls/begin-draft-create-volume-manifest.json",
        "intent-start-draft-part": "controls/start-draft-part-request.json",
        "intent-delete-object": "controls/delete-object-request.json",
        "intent-set-metadata": "controls/set-metadata-route-request.json",
        "intent-abort-operation": "controls/abort-operation-request.json",
        "intent-install-update": "controls/install-update-request.json",
        "intent-acknowledge-ride-imported": "controls/acknowledge-ride-imported-request.json",
    ]

    static func exerciseCanonicalIntent(_ entry: DeviceObjectVectors.Entry, _ json: [String: Any])
        throws
    {
        let bytes = try (json["bytes"] as? String ?? "").hexBytes
        guard let storeId = StoreId(bytes: try (json["storeId"] as? String ?? "").hexBytes) else {
            throw VectorError("\(entry): storeId is not 16 bytes")
        }
        let prefixLength = json["prefixLength"] as? Int ?? 0
        let suffixLength = json["suffixLength"] as? Int ?? 0
        guard prefixLength == CanonicalIntent.prefixBytes, bytes.count == prefixLength + suffixLength
        else { throw VectorError("\(entry): prefix/suffix lengths") }

        // The digest is recomputed here, not read.
        let digest = CanonicalIntent.digest(of: bytes).hexString
        guard digest == json["sha256"] as? String else {
            throw VectorError("\(entry): SHA-256 \(digest) != \(json["sha256"] as? String ?? "")")
        }

        guard let sourcePath = intentSources[entry.name] else {
            throw VectorError("\(entry): no request fixture is mapped to this intent golden")
        }
        let sourceURL = DeviceObjectVectors.suiteDirectory.appendingPathComponent(sourcePath)
        guard let data = FileManager.default.contents(atPath: sourceURL.path),
            let source = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else { throw VectorError("\(entry): source fixture \(sourcePath) missing") }
        let request = try ControlFrame.decode(try (source["frame"] as? String ?? "").hexBytes)
        guard let intent = CanonicalIntent.intent(of: request) else {
            throw VectorError("\(entry): \(sourcePath) carries no canonical intent")
        }
        guard Int(intent.opcode.rawValue) == (json["opcode"] as? [String: Any])?["value"] as? Int
        else { throw VectorError("\(entry): opcode") }
        let rebuilt = try CanonicalIntent.bytes(storeId: storeId, intent: intent)
        guard rebuilt == bytes else {
            throw VectorError("\(entry): rebuilt intent differs\n  \(rebuilt.hexString)")
        }
    }

    // MARK: frame limits

    static func exerciseFrameLimits(_ entry: DeviceObjectVectors.Entry, _ json: [String: Any])
        throws
    {
        guard json["protocolMinimumControlFrame"] as? Int == WireLimits.minimumControlFrame,
            json["protocolMinimumStreamFrame"] as? Int == WireLimits.minimumStreamFrame,
            json["maximumControlFrame"] as? Int == WireLimits.maximumControlFrame,
            json["maximumStreamFrame"] as? Int == WireLimits.maximumStreamFrame
        else { throw VectorError("\(entry): the frozen limits disagree with §1") }

        for testCase in (json["cases"] as? [[String: Any]]) ?? [] {
            let channel = testCase["channel"] as? String ?? ""
            let linkValue = testCase["linkValue"] as? Int ?? 0
            let ceiling = testCase["transportCeiling"] as? Int ?? 0
            let client = testCase["clientMaximum"] as? Int ?? 0
            let device = testCase["deviceMaximum"] as? Int ?? 0
            let outcome: FrameLimitNegotiation.Outcome
            switch channel {
            case "control":
                // §14.1: one ATT value carries ATT_MTU − 3 bytes in either direction.
                guard FrameLimitNegotiation.bleControlCeiling(attMTU: linkValue) == ceiling else {
                    throw VectorError("\(entry): ATT MTU \(linkValue) does not yield \(ceiling)")
                }
                outcome = FrameLimitNegotiation.control(
                    transportCeiling: ceiling, clientMaximum: client, deviceMaximum: device)
            case "stream":
                // §14.0: the CoC SDU *is* the ceiling.
                guard linkValue == ceiling else {
                    throw VectorError("\(entry): SDU \(linkValue) does not yield \(ceiling)")
                }
                outcome = FrameLimitNegotiation.stream(
                    transportCeiling: ceiling, clientMaximum: client, deviceMaximum: device)
            default: throw VectorError("\(entry): unknown channel \(channel)")
            }
            let expected: FrameLimitNegotiation.Outcome
            switch testCase["outcome"] as? String ?? "" {
            case "negotiated": expected = .negotiated(testCase["negotiated"] as? Int ?? -1)
            case "belowProtocolMinimum": expected = .belowProtocolMinimum
            case "undeliverable": expected = .undeliverable
            default: throw VectorError("\(entry): unknown outcome")
            }
            guard outcome == expected else {
                throw VectorError("\(entry): \(channel) \(linkValue) → \(outcome), expected \(expected)")
            }
        }
    }

    // MARK: negatives

    static func exerciseNegative(_ entry: DeviceObjectVectors.Entry, _ json: [String: Any]) throws {
        let bytes = try (json["bytes"] as? String ?? "").hexBytes
        guard let expect = json["expect"] as? [String: Any],
            let categoryName = expect["category"] as? String,
            let categoryValue = expect["categoryValue"] as? Int
        else { throw VectorError("\(entry): no expectation") }
        let detailName = expect["detail"] as? String
        let detailValue = expect["detailValue"] as? Int ?? 0
        let target = json["target"] as? String ?? ""

        do {
            try decodeNegativeTarget(target, bytes)
            throw VectorError("\(entry): decoded, but the vector requires a rejection")
        } catch let fault as WireFault {
            guard Int(fault.category.rawValue) == categoryValue else {
                throw VectorError(
                    "\(entry): category \(fault.category.name)(\(fault.category.rawValue)) != \(categoryName)(\(categoryValue)) — \(fault.context)"
                )
            }
            guard fault.category.name == categoryName else {
                throw VectorError("\(entry): category name \(fault.category.name) != \(categoryName)")
            }
            guard Int(fault.detail) == detailValue else {
                throw VectorError(
                    "\(entry): detail \(fault.detailName ?? "?")(\(fault.detail)) != \(detailName ?? "?")(\(detailValue)) — \(fault.context)"
                )
            }
            guard fault.detailName == detailName else {
                throw VectorError(
                    "\(entry): detail name \(fault.detailName ?? "nil") != \(detailName ?? "nil")")
            }
        }
    }

    private static func decodeNegativeTarget(_ target: String, _ bytes: [UInt8]) throws {
        switch target {
        case "metadataEnvelope": _ = try MetadataEnvelope.decode(bytes, maximumEncodedLength: WireLimits.catalogMetadataCeiling)
        case "errorBody": _ = try ErrorBody.decode(bytes)
        case "capabilities": _ = try CapabilitiesPage.decode(bytes)
        case "subjectEntry": _ = try SubjectEntry.decode(bytes)
        case "configBlock": _ = try DeviceConfigBlock.decode(bytes)
        case "streamFrame": _ = try StreamFrame.decode(bytes)
        case "GetDeviceStatusResponse", "controlFrame": _ = try ControlFrame.decode(bytes)
        case let target where target.hasPrefix("resetStoreEcho("):
            // The label carries the device's mount class, which is what decides whether an all-zero
            // echo is admissible.
            let digits = target.drop { $0 != "=" }.dropFirst().prefix { $0.isNumber }
            guard let raw = UInt8(digits), let mountClass = MountClass(rawValue: raw) else {
                throw VectorError("unparsable mount class in \(target)")
            }
            guard let echo = StoreId(bytes: bytes) else {
                throw VectorError("ResetStore echo is not 16 bytes")
            }
            try ResetStoreEcho.validate(
                echo: echo, currentStoreId: suiteStoreId, mountClass: mountClass)
        case let target where target.hasSuffix("Request") || target.hasSuffix("Response"):
            _ = try ControlFrame.decode(bytes)
        default: throw VectorError("unhandled negative target \(target)")
        }
    }

    // MARK: streams

    static func exerciseStream(_ entry: DeviceObjectVectors.Entry, _ json: [String: Any]) throws {
        let record = try (json["record"] as? String ?? "").hexBytes
        let frame = try StreamFrame.decode(record)
        guard Int(frame.sessionId.rawValue) == json["sessionId"] as? Int else {
            throw VectorError("\(entry): SessionId")
        }
        // §1: `u64` offsets are decimal strings in JSON precisely because they exceed a JS Number.
        guard String(frame.absoluteOffset) == json["offset"] as? String else {
            throw VectorError("\(entry): offset \(frame.absoluteOffset) != \(json["offset"] ?? "")")
        }
        guard Int(frame.direction.rawValue) == json["direction"] as? Int else {
            throw VectorError("\(entry): direction")
        }
        guard Int(frame.flags.rawValue) == json["flags"] as? Int else {
            throw VectorError("\(entry): flags")
        }
        let payloadLength: Int
        switch frame.body {
        case .data(let bytes): payloadLength = bytes.count
        case .fault: payloadLength = StreamFault.bodyBytes
        }
        guard payloadLength == json["payloadLength"] as? Int else {
            throw VectorError("\(entry): payload length")
        }
        guard try frame.encoded() == record else {
            throw VectorError("\(entry): re-encoded record differs")
        }
    }

    // MARK: transcripts

    static func exerciseTranscript(_ entry: DeviceObjectVectors.Entry, _ json: [String: Any]) throws
    {
        guard let events = json["events"] as? [[String: Any]] else {
            throw VectorError("\(entry): no events")
        }
        guard events.count == json["eventCount"] as? Int else {
            throw VectorError("\(entry): eventCount disagrees with the event list")
        }
        for (index, event) in events.enumerated() {
            let channel = event["channel"] as? String ?? ""
            let record = try (event["record"] as? String ?? "").hexBytes
            switch channel {
            case "control":
                guard !record.isEmpty else {
                    throw VectorError("\(entry) event \(index): empty control record")
                }
                let frame = try ControlFrame.decode(record)
                guard try frame.encoded() == record else {
                    throw VectorError("\(entry) event \(index): control re-encode differs")
                }
            case "stream":
                guard !record.isEmpty else {
                    throw VectorError("\(entry) event \(index): empty stream record")
                }
                let frame = try StreamFrame.decode(record)
                guard try frame.encoded() == record else {
                    throw VectorError("\(entry) event \(index): stream re-encode differs")
                }
            case "injected":
                // An injected disconnect/cut/loss carries no bytes by construction.
                guard record.isEmpty else {
                    throw VectorError("\(entry) event \(index): injected event carries a record")
                }
            default: throw VectorError("\(entry) event \(index): unknown channel \(channel)")
            }
        }
    }
}
