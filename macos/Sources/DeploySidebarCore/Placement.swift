// SPDX-License-Identifier: GPL-3.0

import Foundation

/// A corner of the screen the sidebar can sit in.
public enum Corner: String, CaseIterable, Sendable {
    case topLeft = "top-left"
    case topRight = "top-right"
    case bottomLeft = "bottom-left"
    case bottomRight = "bottom-right"

    public var isLeft: Bool { self == .topLeft || self == .bottomLeft }
    public var isTop: Bool { self == .topLeft || self == .topRight }

    /// The corner of a `width` x `height` area nearest a point in it, with y growing downwards.
    public static func nearest(x: Double, y: Double, width: Double, height: Double) -> Corner {
        switch (x < width / 2, y < height / 2) {
        case (true, true): .topLeft
        case (false, true): .topRight
        case (true, false): .bottomLeft
        case (false, false): .bottomRight
        }
    }
}

/// Where the sidebar was dragged to: a corner, and the screen it was dropped on. Saved to the
/// same state file as on Linux, apart from the config so saving settings can't overwrite it.
public struct Placement: Equatable, Sendable {
    public var corner: Corner
    /// The screen's name; `nil` opens on the screen the pointer is on.
    public var output: String?

    public init(corner: Corner = .topRight, output: String? = nil) {
        self.corner = corner
        self.output = output
    }

    /// `$XDG_STATE_HOME/cosmic-deploy-sidebar/placement.toml`.
    public static var path: URL {
        let environment = ProcessInfo.processInfo.environment
        let base = environment["XDG_STATE_HOME"].flatMap { $0.isEmpty ? nil : URL(fileURLWithPath: $0) }
            ?? FileManager.default.homeDirectoryForCurrentUser
                .appendingPathComponent(".local").appendingPathComponent("state")
        return base.appendingPathComponent("cosmic-deploy-sidebar").appendingPathComponent("placement.toml")
    }

    /// The saved placement, or the top right of the screen under the pointer.
    public static func load() -> Placement {
        guard let text = try? String(contentsOf: path, encoding: .utf8) else { return Placement() }
        return parse(text)
    }

    /// Read the state file's TOML, ignoring anything unrecognised.
    public static func parse(_ text: String) -> Placement {
        guard let doc = try? TOML.parse(text) else { return Placement() }
        var placement = Placement()
        if case .string(let corner)? = doc["corner"], let corner = Corner(rawValue: corner) {
            placement.corner = corner
        }
        if case .string(let output)? = doc["output"] {
            placement.output = output
        }
        return placement
    }

    public func toTOML() -> String {
        var text = "corner = \(TOML.quote(corner.rawValue))\n"
        if let output {
            text += "output = \(TOML.quote(output))\n"
        }
        return text
    }

    /// Remember where the sidebar was dropped.
    public func save() throws {
        let path = Self.path
        do {
            try FileManager.default.createDirectory(
                at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
            try toTOML().write(to: path, atomically: true, encoding: .utf8)
        } catch {
            throw AppError("Could not save \(path.path): \(error.localizedDescription)")
        }
    }
}
