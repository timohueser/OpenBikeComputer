import Foundation

/// Failures of the phone-side file-format edge (route import / ride export).
/// `unsupportedFileType` is the H5 state ("OBC imports GPX and TCX route files.").
public enum FormatError: Error, Equatable, Sendable {
    /// No registered decoder/encoder claims this file extension (H5).
    case unsupportedFileType(fileExtension: String)
    /// The file matched a format but its contents don't parse.
    case malformed(reason: String)
}
