// SPDX-License-Identifier: GPL-3.0

import Foundation
import Observation

/// How the sidebar was launched.
public enum LaunchMode: Sendable, Equatable {
    case normal
    /// Show the sidebar now for this service's latest deploy, as if it just went live.
    case preview(String)
    /// Synthetic deploys from the example config, without any network access.
    case demo
}

public enum Phase: Equatable, Sendable {
    case building
    case live(Date)
    case failed
}

/// A deploy being shown in the sidebar.
public struct Watch: Identifiable, Sendable {
    public let service: Int
    public var deploy: Deploy
    public var phase: Phase
    /// The pre-deploy mean of each metric.
    public var baselines: [Double?]
    public var series: [SeriesResult]
    /// Set when the whole metrics request failed, e.g. a bad token.
    public var error: String?

    public var id: String { deploy.id }

    /// The latest value of a metric, when it has one.
    public func current(_ metric: Int) -> Double? {
        guard series.indices.contains(metric), case .success(let points) = series[metric] else { return nil }
        return points.last?.value
    }

    /// The pre-deploy mean of a metric, once fetched.
    public func baseline(_ metric: Int) -> Double? {
        baselines.indices.contains(metric) ? baselines[metric] : nil
    }
}

/// The header line of a watch card, e.g. "live · 4m 05s left".
public struct WatchStatus: Equatable, Sendable {
    public enum Tone: Equatable, Sendable {
        case accent
        case level(Level)
    }

    public let text: String
    public let tone: Tone
}

/// The sidebar's state: idle until a watched service deploys, then each metric against its
/// pre-deploy baseline until the window ends, then a fade out.
@MainActor @Observable
public final class SidebarModel {
    public static let fadeIn: TimeInterval = 0.25
    public static let fadeOut: TimeInterval = 3
    /// How much history before the deploy the sparklines include, so the restart is visible.
    static let leadIn: TimeInterval = 5 * 60
    static let step: TimeInterval = 60

    public private(set) var config: Config?
    public private(set) var watches: [Watch] = []
    /// A setup problem, shown in the sidebar until dismissed.
    public private(set) var notice: String?
    public private(set) var fadeStarted: Date?
    /// Updated every second, for the countdowns.
    public private(set) var now = Date()
    public let isDemo: Bool

    /// Called whenever the panel may need to appear, resize, fade or go away.
    @ObservationIgnored public var onSurfaceChange: (@MainActor () -> Void)?
    /// The config file's mtime when last read, to reload after it is edited.
    @ObservationIgnored private var configModified: Date?
    @ObservationIgnored private var renderKey: String?
    @ObservationIgnored private var grafana: Grafana?
    @ObservationIgnored private var preview: String?
    /// The newest deploy ID seen per service, to spot new ones.
    @ObservationIgnored private var lastSeen: [Int: String] = [:]
    @ObservationIgnored private var lastRenderPoll = Date.distantPast
    @ObservationIgnored private var lastMetricsPoll = Date.distantPast
    @ObservationIgnored private var lastConfigCheck = Date()
    @ObservationIgnored private var heartbeat: Task<Void, Never>?
    /// The time, replaceable so tests can skip ahead.
    @ObservationIgnored var clock: @MainActor () -> Date = { Date() }
    /// Off in tests, so a real config file never leaks in.
    @ObservationIgnored private var watchesConfigFile: Bool

    /// A model that does nothing until `start()`.
    public init(mode: LaunchMode) {
        isDemo = mode == .demo
        watchesConfigFile = mode != .demo
        if case .preview(let service) = mode {
            preview = service
        }
    }

    /// A model with the given setup, for tests.
    init(setup: Setup) {
        isDemo = false
        watchesConfigFile = false
        apply(setup)
    }

    /// Whether there is anything to show.
    public var isShowing: Bool {
        notice != nil || !watches.isEmpty
    }

    /// Load the config and credentials (or the demo data), then poll once a second.
    public func start() {
        guard heartbeat == nil else { return }
        if isDemo {
            loadDemo()
        } else {
            configModified = Config.modified()
            apply(Setup.load())
        }
        syncSurface()
        tick()
        heartbeat = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                self?.tick()
            }
        }
    }

    /// Show a service's latest deploy now, as if it just went live.
    public func previewLatest(_ service: String) {
        preview = service
        pollRender()
    }

    /// Hide a deploy.
    public func dismiss(_ deployID: String) {
        watches.removeAll { $0.deploy.id == deployID }
        syncSurface()
    }

    /// Hide the setup notice until the config next changes.
    public func dismissNotice() {
        notice = nil
        syncSurface()
    }

    /// Pick up a changed config. Watches survive unless the watched services changed.
    public func reloadIfChanged(force: Bool = false) {
        guard watchesConfigFile else { return }
        let modified = Config.modified()
        guard force || modified != configModified else { return }
        configModified = modified

        let setup = Setup.load()
        func serviceIDs(_ config: Config?) -> [String] {
            config?.services.map(\.renderServiceID) ?? []
        }
        if serviceIDs(config) != serviceIDs(setup.config) {
            watches.removeAll()
            lastSeen.removeAll()
        }
        apply(setup)
        syncSurface()
        pollRender()
    }

    public func serviceName(_ service: Int) -> String {
        guard let services = config?.services, services.indices.contains(service) else { return "?" }
        return services[service].name
    }

    public func metrics(_ service: Int) -> [Metric] {
        guard let services = config?.services, services.indices.contains(service) else { return [] }
        return services[service].metrics
    }

    /// The most severe level across a watch's metrics.
    public func worstLevel(_ watch: Watch) -> Level {
        metrics(watch.service).enumerated()
            .map { index, metric in
                Health.assess(metric, baseline: watch.baseline(index), current: watch.current(index))
            }
            .max() ?? .unknown
    }

    /// What a watch's header says, coloured by how it is going.
    public func status(_ watch: Watch) -> WatchStatus {
        switch watch.phase {
        case .building:
            return WatchStatus(
                text: "deploying · \(Health.duration(now.timeIntervalSince(watch.deploy.createdAt)))", tone: .accent)
        case .failed:
            return WatchStatus(text: "deploy failed", tone: .level(.bad))
        case .live(let at):
            let window = TimeInterval((config?.windowMinutes ?? 10) * 60)
            let left = window - now.timeIntervalSince(at)
            let text = left > 0 ? "live · \(Health.duration(left)) left" : "live · held, something looks off"
            return WatchStatus(text: text, tone: .level(worstLevel(watch)))
        }
    }

    private func apply(_ setup: Setup) {
        config = setup.config
        renderKey = setup.renderKey
        grafana = setup.grafana
        notice = setup.notice
    }

    private func syncSurface() {
        onSurfaceChange?()
    }

    func tick() {
        now = clock()
        if watchesConfigFile, now.timeIntervalSince(lastConfigCheck) >= 3 {
            lastConfigCheck = now
            reloadIfChanged()
        }
        if let config, renderKey != nil,
           now.timeIntervalSince(lastRenderPoll) >= TimeInterval(max(config.renderPollSeconds, 5)) {
            pollRender()
        }
        if let config, watches.contains(where: { $0.isLive }),
           now.timeIntervalSince(lastMetricsPoll) >= TimeInterval(max(config.metricsPollSeconds, 15)) {
            lastMetricsPoll = now
            for watch in watches where watch.isLive {
                fetchMetrics(deployID: watch.deploy.id, baseline: false)
            }
        }
        if isShowing {
            sweep()
        }
        if let fadeStarted, now.timeIntervalSince(fadeStarted) >= Self.fadeOut {
            watches.removeAll()
            self.fadeStarted = nil
            syncSurface()
        }
    }

    private func pollRender() {
        lastRenderPoll = clock()
        guard let config, let renderKey else { return }
        for (index, service) in config.services.enumerated() {
            let serviceID = service.renderServiceID
            Task {
                do {
                    guard let deploy = try await Render.latestDeploy(apiKey: renderKey, serviceID: serviceID),
                          // The config may have changed while this was in flight.
                          self.config?.services.indices.contains(index) == true,
                          self.config?.services[index].renderServiceID == serviceID
                    else { return }
                    deployFetched(service: index, deploy: deploy)
                } catch {
                    log.error("Render: \(error.localizedDescription, privacy: .public)")
                }
            }
        }
    }

    func deployFetched(service: Int, deploy: Deploy) {
        let previous = lastSeen.updateValue(deploy.id, forKey: service)

        if let preview, preview == serviceName(service) {
            self.preview = nil
            startWatch(service: service, deploy: deploy, phase: .live(clock()))
            return
        }

        let existing = watches.first { $0.deploy.id == deploy.id }?.phase
        switch (previous, existing, deploy.status) {
        // On startup only a deploy already under way is worth showing.
        case (nil, _, .building):
            startWatch(service: service, deploy: deploy, phase: .building)
        case (nil, _, _):
            break
        case (let previous?, _, let status) where previous != deploy.id:
            switch status {
            case .building:
                startWatch(service: service, deploy: deploy, phase: .building)
            case .failed:
                startWatch(service: service, deploy: deploy, phase: .failed)
            // It went live between polls.
            case .live:
                startWatch(service: service, deploy: deploy, phase: .live(deploy.finishedAt ?? clock()))
            case .canceled, .superseded:
                break
            }
        case (_, .building?, .live):
            startWatch(service: service, deploy: deploy, phase: .live(deploy.finishedAt ?? clock()))
        case (_, .building?, .failed):
            startWatch(service: service, deploy: deploy, phase: .failed)
        case (_, _?, .canceled), (_, _?, .superseded):
            watches.removeAll { $0.deploy.id == deploy.id }
            syncSurface()
        default:
            break
        }
    }

    /// Show a deploy, replacing anything already shown for its service.
    private func startWatch(service: Int, deploy: Deploy, phase: Phase) {
        let count = metrics(service).count
        // Keep baselines across the Building -> Live transition.
        let baselines = watches.first { $0.deploy.id == deploy.id }?.baselines ?? Array(repeating: nil, count: count)

        watches.removeAll { $0.service == service }
        watches.append(Watch(
            service: service,
            deploy: deploy,
            phase: phase,
            baselines: baselines,
            series: Array(repeating: .success([]), count: count),
            error: nil
        ))
        watches.sort { $0.service < $1.service }
        fadeStarted = nil

        syncSurface()
        if case .live = phase {
            fetchMetrics(deployID: deploy.id, baseline: true)
            fetchMetrics(deployID: deploy.id, baseline: false)
        }
    }

    /// Query every metric for a deploy: the pre-deploy baseline, or the window around it.
    private func fetchMetrics(deployID: String, baseline: Bool) {
        guard let grafana, let config, let watch = watches.first(where: { $0.deploy.id == deployID }) else { return }
        let exprs = metrics(watch.service).map(\.expr)
        guard !exprs.isEmpty else { return }

        let started = watch.deploy.createdAt
        let from = baseline ? started.addingTimeInterval(-TimeInterval(config.baselineMinutes * 60))
            : started.addingTimeInterval(-Self.leadIn)
        let to = baseline ? started : clock()
        Task {
            do {
                let series = try await grafana.queryRange(exprs, from: from, to: to, step: Self.step)
                metricsFetched(deployID: deployID, baseline: baseline, result: .success(series))
            } catch {
                metricsFetched(deployID: deployID, baseline: baseline, result: .failure(AppError(error.localizedDescription)))
            }
        }
    }

    func metricsFetched(deployID: String, baseline: Bool, result: Result<[SeriesResult], AppError>) {
        guard let index = watches.firstIndex(where: { $0.deploy.id == deployID }) else { return }
        switch result {
        case .success(let series) where baseline:
            watches[index].baselines = series.map { result in
                guard case .success(let points) = result else { return nil }
                return Health.mean(points.map(\.value))
            }
        case .success(let series):
            watches[index].series = series
            watches[index].error = nil
        case .failure(let error):
            watches[index].error = error.message
        }
        syncSurface()
    }

    /// Drop watches whose window has passed, fading out when that leaves nothing to show.
    private func sweep() {
        guard let config else { return }
        let window = TimeInterval(config.windowMinutes * 60)
        let now = clock()
        func finished(_ watch: Watch) -> Bool {
            guard case .live(let at) = watch.phase else { return false }
            return now.timeIntervalSince(at) >= window && worstLevel(watch) < .bad
        }

        if notice == nil, !watches.isEmpty, watches.allSatisfy(finished) {
            if fadeStarted == nil {
                fadeStarted = now
                syncSurface()
            }
            return
        }
        if fadeStarted != nil {
            fadeStarted = nil
        }
        if watches.contains(where: finished) {
            watches.removeAll(where: finished)
        }
        syncSurface()
    }

    /// One live deploy with a latency regression and one still building, with no credentials.
    private func loadDemo() {
        guard let config = try? Config.parse(exampleConfig) else { return }
        let now = Date()
        let liveAt = now.addingTimeInterval(-3 * 60)
        let started = liveAt.addingTimeInterval(-4 * 60)
        func deploy(_ id: String, _ message: String) -> Deploy {
            Deploy(
                id: id, status: .live, revision: "23675c8", commitMessage: message, createdAt: started,
                finishedAt: liveAt, url: "https://dashboard.render.com")
        }

        let api = config.services[0].metrics
        // Per metric: the pre-deploy level and the multiplier after it, p95 latency regressing.
        let shapes = [1.0, 1.9, 1.1, 1.0, 1.3, 1.05]
        let baselines: [Double?] = [140.0, 0.18, 0.02, 0.0, 0.03, 6.0e8]
        let series: [SeriesResult] = zip(baselines, shapes).prefix(api.count).map { baseline, after in
            .success((0..<13).map { minute in
                let time = started.addingTimeInterval(-Self.leadIn + Double(minute) * 60)
                let wobble = 1 + 0.06 * sin(Double(minute) * 1.7)
                let level = time < liveAt ? 1 : after
                return Point(time: time, value: (baseline ?? 0) * level * wobble)
            })
        }

        var building = deploy("dep-demo-app", "Tidy SSR logging")
        building.status = .building
        building.createdAt = now.addingTimeInterval(-95)
        building.finishedAt = nil

        self.config = config
        watches = [
            Watch(
                service: 0, deploy: deploy("dep-demo-api", "Merge pull request #5871\n\nCalendar playlist"),
                phase: .live(liveAt), baselines: baselines, series: series, error: nil),
            Watch(service: 1, deploy: building, phase: .building, baselines: [], series: [], error: nil),
        ]
    }
}

extension Watch {
    var isLive: Bool {
        if case .live = phase { true } else { false }
    }
}

/// Config and credentials, plus the first problem stopping the sidebar from working.
struct Setup {
    var config: Config?
    var renderKey: String?
    var grafana: Grafana?
    var notice: String?

    static func load() -> Setup {
        var setup = Setup()
        do {
            setup.config = try Config.load()
        } catch {
            setup.notice = error.localizedDescription
        }
        setup.renderKey = Keychain.renderAPIKey()
        let token = Keychain.grafanaToken()
        if setup.notice == nil, setup.renderKey == nil {
            setup.notice = "No Render API key yet."
        }
        if setup.notice == nil, token == nil {
            setup.notice = "No Grafana token yet."
        }
        if let config = setup.config, let token {
            setup.grafana = Grafana(url: config.grafanaURL, token: token, datasourceUID: config.datasourceUID)
        }
        return setup
    }
}
