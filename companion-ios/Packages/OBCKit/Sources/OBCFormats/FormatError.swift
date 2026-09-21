import Foundation

/// Failures of the phone-side file-format edge: route import and ride export.
public enum FormatError: Error, Equatable, Sendable {
    /// No registered decoder or encoder claims this file extension.
    case unsupportedFileType(fileExtension: String)
    /// The file matched a format but its contents don't parse.
    case malformed(reason: String)
}
