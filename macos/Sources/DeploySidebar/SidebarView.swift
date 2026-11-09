// SPDX-License-Identifier: GPL-3.0

import AppKit
import DeploySidebarCore
import SwiftUI

/// Everything in the panel: a setup notice, then a card per deploy.
struct SidebarView: View {
    static let width: CGFloat = 380

    let model: SidebarModel
    let openSettings: @MainActor () -> Void
    let onResize: @MainActor (CGSize) -> Void

    var body: some View {
        VStack(spacing: 8) {
            if let notice = model.notice {
                NoticeCard(notice: notice, openSettings: openSettings, dismiss: model.dismissNotice)
            }
            ForEach(model.watches) { watch in
                WatchCard(model: model, watch: watch)
            }
        }
        .padding(8)
        .frame(width: Self.width)
        .background(PanelBackground(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color(nsColor: .separatorColor)))
        .fixedSize(horizontal: false, vertical: true)
        .onGeometryChange(for: CGSize.self, of: \.size) { size in
            onResize(size)
        }
    }
}

private struct NoticeCard: View {
    let notice: String
    let openSettings: @MainActor () -> Void
    let dismiss: @MainActor () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Deploy Sidebar")
                .font(.headline)
                .foregroundStyle(.orange)
            Text(notice)
                .font(.callout)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 12) {
                LinkButton("Open settings", color: .accentColor, action: openSettings)
                LinkButton("Dismiss", color: .secondary, action: dismiss)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardStyle()
    }
}

private struct WatchCard: View {
    let model: SidebarModel
    let watch: Watch

    var body: some View {
        let status = model.status(watch)
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Circle()
                    .fill(status.tone.color)
                    .frame(width: 8, height: 8)
                Text(model.serviceName(watch.service))
                    .font(.headline)
                Spacer(minLength: 4)
                Text(status.text)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
                Button {
                    model.dismiss(watch.deploy.id)
                } label: {
                    Image(systemName: "xmark")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help("Dismiss")
            }

            LinkButton(commitLine, color: .secondary) {
                if let url = URL(string: watch.deploy.url) {
                    NSWorkspace.shared.open(url)
                }
            }
            .help("Open in Render")

            if case .live(let liveAt) = watch.phase {
                if let error = watch.error {
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
                ForEach(Array(model.metrics(watch.service).enumerated()), id: \.offset) { index, metric in
                    MetricRow(metric: metric, series: watch.series.indices.contains(index) ? watch.series[index] : nil,
                              baseline: watch.baseline(index), liveAt: liveAt)
                }
            }
        }
        .cardStyle()
    }

    private var commitLine: String {
        switch (watch.deploy.revision, watch.deploy.commitSummary) {
        case let (revision?, summary?): "\(revision) · \(summary)"
        case let (revision?, nil): revision
        case let (nil, summary?): summary
        case (nil, nil): "Open in Render"
        }
    }
}

/// One metric: label, sparkline, current value and its change from the baseline.
private struct MetricRow: View {
    let metric: Metric
    let series: SeriesResult?
    let baseline: Double?
    let liveAt: Date

    var body: some View {
        let points = (try? series?.get()) ?? []
        let current = points.last?.value
        let level = Health.assess(metric, baseline: baseline, current: current)
        let value = if let current {
            Health.format(metric.unit, current)
        } else if case .failure = series {
            "error"
        } else {
            "…"
        }
        let delta = baseline.flatMap { baseline in
            current.flatMap { Health.delta(baseline: baseline, current: $0) }
        } ?? ""

        HStack(spacing: 8) {
            Text(metric.label)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            Sparkline(points: points, baseline: baseline, liveAt: liveAt, color: level.color)
            Text(value)
                .frame(width: 62, alignment: .trailing)
            Text(delta)
                // Normal movement stays quiet; only a warning or problem is coloured.
                .foregroundStyle(level >= .warn ? level.color : .secondary)
                .frame(width: 40, alignment: .trailing)
        }
        .font(.system(size: 11))
        .monospacedDigit()
        .help(failureMessage ?? metric.expr)
    }

    private var failureMessage: String? {
        if case .failure(let error) = series { error.message } else { nil }
    }
}

/// Bars for the latest points, dimmed before the deploy went live, scaled with the baseline.
private struct Sparkline: View {
    static let bars = 16
    static let height: CGFloat = 18

    let points: [Point]
    let baseline: Double?
    let liveAt: Date
    let color: Color

    var body: some View {
        let shown = points.suffix(Self.bars)
        let max = (shown.map(\.value) + [baseline ?? 0]).max() ?? 0
        HStack(alignment: .bottom, spacing: 2) {
            ForEach(0..<(Self.bars - shown.count), id: \.self) { _ in
                Color.clear.frame(width: 3, height: 1)
            }
            ForEach(Array(shown.enumerated()), id: \.offset) { _, point in
                let fraction = max > 0 ? CGFloat(point.value / max) : 0
                RoundedRectangle(cornerRadius: 1)
                    .fill(point.time < liveAt ? Color.secondary.opacity(0.5) : color)
                    .frame(width: 3, height: 2 + fraction * (Self.height - 2))
            }
        }
        .frame(height: Self.height, alignment: .bottom)
    }
}

/// Text that acts as a link, with a pointing hand on hover.
private struct LinkButton: View {
    let title: String
    let color: Color
    let action: @MainActor () -> Void

    init(_ title: String, color: Color, action: @escaping @MainActor () -> Void) {
        self.title = title
        self.color = color
        self.action = action
    }

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.caption)
                .foregroundStyle(color)
                .lineLimit(1)
                .truncationMode(.tail)
        }
        .buttonStyle(.plain)
        .onHover { inside in
            if inside { NSCursor.pointingHand.push() } else { NSCursor.pop() }
        }
    }
}

/// The translucent background of the whole panel, blurring what is behind it.
private struct PanelBackground: NSViewRepresentable {
    let cornerRadius: CGFloat

    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .popover
        view.blendingMode = .behindWindow
        view.state = .active
        view.maskImage = Self.mask(cornerRadius: cornerRadius)
        return view
    }

    func updateNSView(_ view: NSVisualEffectView, context: Context) {}

    /// A stretchable rounded rectangle, so the blur has rounded corners at any size.
    private static func mask(cornerRadius: CGFloat) -> NSImage {
        let edge = 2 * cornerRadius + 1
        let image = NSImage(size: NSSize(width: edge, height: edge), flipped: false) { rect in
            NSColor.black.setFill()
            NSBezierPath(roundedRect: rect, xRadius: cornerRadius, yRadius: cornerRadius).fill()
            return true
        }
        image.capInsets = NSEdgeInsets(top: cornerRadius, left: cornerRadius, bottom: cornerRadius, right: cornerRadius)
        image.resizingMode = .stretch
        return image
    }
}

extension View {
    fileprivate func cardStyle() -> some View {
        padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 8))
    }
}

extension Level {
    var color: Color {
        switch self {
        case .unknown: .secondary
        case .ok: .green
        case .warn: .orange
        case .bad: .red
        }
    }
}

extension WatchStatus.Tone {
    var color: Color {
        switch self {
        case .accent: .accentColor
        case .level(let level): level.color
        }
    }
}
