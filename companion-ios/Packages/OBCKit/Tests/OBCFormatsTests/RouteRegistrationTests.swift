import Foundation
import Testing
import OBCFormats

@Test func routeImportTypesMatchCompanionRegistration() throws {
    let source = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .appending(path: "../../../../OBCCompanion/Info.plist")
        .standardizedFileURL
    let data = try Data(contentsOf: source)
    let plist = try #require(PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any])
    let declarations = try #require(plist["UTImportedTypeDeclarations"] as? [[String: Any]])
    let documentTypes = try #require(plist["CFBundleDocumentTypes"] as? [[String: Any]])

    let extensionsByType = Dictionary(uniqueKeysWithValues: try declarations.map { declaration in
        let identifier = try #require(declaration["UTTypeIdentifier"] as? String)
        let tags = try #require(declaration["UTTypeTagSpecification"] as? [String: Any])
        let extensions = try #require(tags["public.filename-extension"] as? [String])
        return (identifier, Set(extensions))
    })
    let registeredTypes = Set(try documentTypes.flatMap { documentType in
        try #require(documentType["LSItemContentTypes"] as? [String])
    })

    #expect(registeredTypes == ["com.topografix.gpx", "com.garmin.tcx"])
    #expect(Set(extensionsByType.keys) == registeredTypes)
    let registeredExtensions = registeredTypes.reduce(into: Set<String>()) { result, type in
        result.formUnion(extensionsByType[type] ?? [])
    }
    let importer = RouteImporter(decoders: [GPXRouteDecoder(), TCXRouteDecoder()])
    #expect(registeredExtensions == importer.supportedFileExtensions)
}
