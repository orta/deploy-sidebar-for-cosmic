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
}

/// Logs for Console.app, or `make logs`.
public let log = Logger(subsystem: "io.orta.DeploySidebar", category: "sidebar")
