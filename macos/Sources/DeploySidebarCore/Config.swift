// SPDX-License-Identifier: GPL-3.0

import Foundation

/// The TOML config describing which services to watch and what to show for each.
/// It is the same file and format as the COSMIC version, so one config works on both.
public struct Config: Equatable, Sendable {
    public var grafanaURL: String
    public var datasourceUID: String
    public var windowMinutes = 10
    public var baselineMinutes = 30
    public var renderPollSeconds = 20
    public var metricsPollSeconds = 60
    public var services: [Service]

    /// Make a config, with the default timings unless given.
    public init(
        grafanaURL: String, datasourceUID: String, windowMinutes: Int = 10, baselineMinutes: Int = 30,
        renderPollSeconds: Int = 20, metricsPollSeconds: Int = 60, services: [Service]
    ) {
        self.grafanaURL = grafanaURL
        self.datasourceUID = datasourceUID
        self.windowMinutes = windowMinutes
        self.baselineMinutes = baselineMinutes
        self.renderPollSeconds = renderPollSeconds
        self.metricsPollSeconds = metricsPollSeconds
        self.services = services
    }
}

public struct Service: Equatable, Sendable {
    public var name: String
    public var renderServiceID: String
    public var metrics: [Metric]

    /// Make a service.
    public init(name: String, renderServiceID: String, metrics: [Metric] = []) {
        self.name = name
        self.renderServiceID = renderServiceID
        self.metrics = metrics
    }
}

public struct Metric: Equatable, Sendable {
    public var label: String
    public var expr: String
    public var unit: MetricUnit
    public var worse: Direction
    /// Values below this are never flagged, however they compare to the baseline.
    public var floor: Double

    /// Make a metric.
    public init(label: String, expr: String, unit: MetricUnit, worse: Direction, floor: Double = 0) {
        self.label = label
        self.expr = expr
        self.unit = unit
        self.worse = worse
        self.floor = floor
    }
}

public enum MetricUnit: String, CaseIterable, Sendable {
    case count
    case perSecond = "per_second"
    case seconds
    case bytes
    case ratio

    /// The name shown in the settings window.
    public var label: String {
        switch self {
        case .count: "Count"
        case .perSecond: "Per second"
        case .seconds: "Seconds"
        case .bytes: "Bytes"
        case .ratio: "Ratio"
        }
    }
}

/// The direction in which a metric moving means trouble.
public enum Direction: String, CaseIterable, Sendable {
    case higher
    case lower

    /// The name shown in the settings window.
    public var label: String {
        switch self {
        case .higher: "Higher is worse"
        case .lower: "Lower is worse"
        }
    }
}

private let placeholder = "REPLACE_ME"

extension Config {
    /// Where the config lives: `$XDG_CONFIG_HOME/cosmic-deploy-sidebar/config.toml`, as on Linux.
    public static var path: URL {
        let environment = ProcessInfo.processInfo.environment
        let base = environment["XDG_CONFIG_HOME"].flatMap { $0.isEmpty ? nil : URL(fileURLWithPath: $0) }
            ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".config")
        return base.appendingPathComponent("cosmic-deploy-sidebar").appendingPathComponent("config.toml")
    }

    /// The config path with the home directory shortened to `~`, for messages.
    public static var displayPath: String {
        (path.path as NSString).abbreviatingWithTildeInPath
    }

    /// Read and validate the config, describing what is wrong when it can't be used.
    public static func load() throws -> Config {
        guard FileManager.default.fileExists(atPath: path.path) else {
            throw AppError("No config yet at \(displayPath). Open settings from the menu bar to set one up.")
        }
        let config = try read()
        if let problem = config.validationProblem() {
            throw AppError(problem)
        }
        return config
    }

    /// The saved config for editing, or the example with its placeholders blanked when there is none.
    public static func loadForEditing() throws -> Config {
        if FileManager.default.fileExists(atPath: path.path) {
            return try read()
        }
        var config = try parse(exampleConfig)
        config.grafanaURL = ""
        config.datasourceUID = ""
        for index in config.services.indices where config.services[index].renderServiceID == placeholder {
            config.services[index].renderServiceID = ""
        }
        return config
    }

    private static func read() throws -> Config {
        let text: String
        do {
            text = try String(contentsOf: path, encoding: .utf8)
        } catch {
            throw AppError("Could not read \(displayPath): \(error.localizedDescription)")
        }
        do {
            return try parse(text)
        } catch {
            throw AppError("\(displayPath) is invalid: \(error.localizedDescription)")
        }
    }

    /// Write the config, replacing any comments (the example config documents every field).
    public func save() throws {
        let directory = Self.path.deletingLastPathComponent()
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try toTOML().write(to: Self.path, atomically: true, encoding: .utf8)
        } catch {
            throw AppError("Could not save \(Self.displayPath): \(error.localizedDescription)")
        }
    }

    /// When the config file last changed, to notice edits made outside the app.
    public static func modified() -> Date? {
        try? FileManager.default.attributesOfItem(atPath: path.path)[.modificationDate] as? Date
    }

    /// The first field a running sidebar can't do without, or nil when it has everything.
    public func validationProblem() -> String? {
        func missing(_ value: String) -> Bool {
            value.trimmingCharacters(in: .whitespaces).isEmpty || value == placeholder
        }
        if missing(grafanaURL) {
            return "Set the Grafana URL in settings"
        }
        if missing(datasourceUID) {
            return "Pick a Prometheus datasource in settings"
        }
        if let service = services.first(where: { missing($0.renderServiceID) }) {
            return "Pick a Render service for \(service.name) in settings"
        }
        return nil
    }

    /// Parse config TOML, with serde-style messages for missing or mistyped fields.
    public static func parse(_ text: String) throws -> Config {
        let root = Fields(table: try TOML.parse(text), context: "")
        return Config(
            grafanaURL: try root.string("grafana_url"),
            datasourceUID: try root.string("datasource_uid"),
            windowMinutes: try root.int("window_minutes") ?? 10,
            baselineMinutes: try root.int("baseline_minutes") ?? 30,
            renderPollSeconds: try root.int("render_poll_seconds") ?? 20,
            metricsPollSeconds: try root.int("metrics_poll_seconds") ?? 60,
            services: try root.tables("services", required: true).map { service in
                Service(
                    name: try service.string("name"),
                    renderServiceID: try service.string("render_service_id"),
                    metrics: try service.tables("metrics", required: false).map { metric in
                        Metric(
                            label: try metric.string("label"),
                            expr: try metric.string("expr"),
                            unit: try metric.choice("unit"),
                            worse: try metric.choice("worse"),
                            floor: try metric.double("floor") ?? 0
                        )
                    }
                )
            }
        )
    }

    /// The config as TOML, laid out like the example config.
    public func toTOML() -> String {
        var lines = [
            "# Written by Deploy Sidebar settings. Field docs: config.example.toml in the repo.",
            "",
            "grafana_url = \(TOML.quote(grafanaURL))",
            "datasource_uid = \(TOML.quote(datasourceUID))",
            "window_minutes = \(windowMinutes)",
            "baseline_minutes = \(baselineMinutes)",
            "render_poll_seconds = \(renderPollSeconds)",
            "metrics_poll_seconds = \(metricsPollSeconds)",
        ]
        for service in services {
            lines += [
                "",
                "[[services]]",
                "name = \(TOML.quote(service.name))",
                "render_service_id = \(TOML.quote(service.renderServiceID))",
            ]
            for metric in service.metrics {
                lines += [
                    "",
                    "[[services.metrics]]",
                    "label = \(TOML.quote(metric.label))",
                    "expr = \(TOML.quote(metric.expr))",
                    "unit = \(TOML.quote(metric.unit.rawValue))",
                    "worse = \(TOML.quote(metric.worse.rawValue))",
                    "floor = \(TOML.format(metric.floor))",
                ]
            }
        }
        return lines.joined(separator: "\n") + "\n"
    }
}

/// Typed reads from a TOML table, naming the field and where it is when one is wrong.
private struct Fields {
    let table: [String: TOMLValue]
    let context: String

    func fail(_ message: String) -> AppError {
        AppError(context.isEmpty ? message : "\(context): \(message)")
    }

    func string(_ key: String) throws -> String {
        switch table[key] {
        case .string(let value): return value
        case nil: throw fail("missing field `\(key)`")
        default: throw fail("`\(key)` should be a string")
        }
    }

    func int(_ key: String) throws -> Int? {
        switch table[key] {
        case .integer(let value): return value
        case nil: return nil
        default: throw fail("`\(key)` should be a whole number")
        }
    }

    /// A float, also accepting a whole number such as `floor = 5`.
    func double(_ key: String) throws -> Double? {
        switch table[key] {
        case .float(let value): return value
        case .integer(let value): return Double(value)
        case nil: return nil
        default: throw fail("`\(key)` should be a number")
        }
    }

    func choice<Choice: RawRepresentable & CaseIterable>(_ key: String) throws -> Choice
    where Choice.RawValue == String {
        let raw = try string(key)
        guard let value = Choice(rawValue: raw) else {
            let expected = Choice.allCases.map(\.rawValue).joined(separator: ", ")
            throw fail("unknown \(key) \"\(raw)\", expected one of \(expected)")
        }
        return value
    }

    func tables(_ key: String, required: Bool) throws -> [Fields] {
        switch table[key] {
        case nil where !required:
            return []
        case nil:
            throw fail("missing field `\(key)`")
        case .array(let items):
            return try items.enumerated().map { index, item in
                guard case .table(let table) = item else { throw fail("`\(key)` should be tables") }
                let name = context.isEmpty ? key : "\(context).\(key)"
                return Fields(table: table, context: "\(name)[\(index)]")
            }
        default:
            throw fail("`\(key)` should be an array of tables, written [[\(key)]]")
        }
    }
}
