// SPDX-License-Identifier: GPL-3.0

import Foundation

public enum Level: Int, Comparable, Sendable {
    case unknown
    case ok
    case warn
    case bad

    public static func < (lhs: Level, rhs: Level) -> Bool {
        lhs.rawValue < rhs.rawValue
    }
}

/// Judging a metric against its pre-deploy baseline, and formatting its values.
public enum Health {
    /// How far past the baseline (in the bad direction) counts as a warning.
    static let warnRatio = 1.25
    /// How far past the baseline counts as a problem worth keeping the sidebar up for.
    static let badRatio = 1.5

    /// Compare the current value to the baseline, in the direction the metric is bad in.
    public static func assess(_ metric: Metric, baseline: Double?, current: Double?) -> Level {
        guard let baseline, let current else { return .unknown }

        let ratio: Double
        switch metric.worse {
        case .higher:
            if current <= metric.floor { return .ok }
            ratio = current / max(baseline, metric.floor, .leastNormalMagnitude)
        case .lower:
            // Too little traffic before the deploy to say a drop means anything.
            if baseline <= metric.floor { return .ok }
            ratio = baseline / max(current, .leastNormalMagnitude)
        }

        if ratio >= badRatio { return .bad }
        if ratio >= warnRatio { return .warn }
        return .ok
    }

    /// The mean of a series, or nil when it is empty.
    public static func mean(_ values: [Double]) -> Double? {
        values.isEmpty ? nil : values.reduce(0, +) / Double(values.count)
    }

    /// Format a value for display in its unit.
    public static func format(_ unit: MetricUnit, _ value: Double) -> String {
        switch unit {
        case .count: return compact(value)
        case .perSecond: return "\(compact(value))/s"
        case .ratio: return String(format: "%.1f%%", value * 100)
        case .seconds where value < 1: return String(format: "%.0fms", value * 1000)
        case .seconds: return String(format: "%.2fs", value)
        case .bytes:
            let mb = value / (1024 * 1024)
            return mb >= 1024 ? String(format: "%.1fGB", mb / 1024) : String(format: "%.0fMB", mb)
        }
    }

    /// The current value relative to the baseline, e.g. "+12%".
    public static func delta(baseline: Double, current: Double) -> String? {
        guard abs(baseline) >= .ulpOfOne else { return nil }
        let change = (current - baseline) / baseline * 100
        return String(format: "%+.0f%%", change)
    }

    static func compact(_ value: Double) -> String {
        if value >= 1000 { return String(format: "%.1fk", value / 1000) }
        if value >= 10 { return String(format: "%.0f", value) }
        return String(format: "%.2f", value)
    }

    /// Elapsed or remaining time, e.g. "4m 05s" or "40s".
    public static func duration(_ interval: TimeInterval) -> String {
        let seconds = max(0, Int(interval))
        return seconds >= 60 ? String(format: "%dm %02ds", seconds / 60, seconds % 60) : "\(seconds)s"
    }
}
