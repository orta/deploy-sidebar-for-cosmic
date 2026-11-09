// SPDX-License-Identifier: GPL-3.0

import Foundation
import os

/// A failure with a message ready to show in the UI.
public struct AppError: Error, LocalizedError, Sendable, Equatable {
    public let message: String

    /// Wrap a message.
    public init(_ message: String) {
        self.message = message
    }

    public var errorDescription: String? { message }

    /// An error in a sentence. JSON decoding errors become e.g. "`uid` is missing at datasources.Prometheus".
    static func describe(_ error: Error) -> String {
        guard let error = error as? DecodingError else { return error.localizedDescription }
        func path(_ context: DecodingError.Context) -> String {
            let path = context.codingPath.map { $0.intValue.map(String.init) ?? $0.stringValue }.joined(separator: ".")
            return path.isEmpty ? "the top level" : path
        }
        switch error {
        case .keyNotFound(let key, let context): return "`\(key.stringValue)` is missing at \(path(context))"
        case .typeMismatch(let type, let context): return "\(path(context)) isn't a \(type)"
        case .valueNotFound(let type, let context): return "\(path(context)) is null instead of a \(type)"
        case .dataCorrupted(let context): return context.debugDescription
        @unknown default: return error.localizedDescription
        }
    }
}

/// Logs for Console.app, or `make logs`.
public let log = Logger(subsystem: "io.orta.DeploySidebar", category: "sidebar")
