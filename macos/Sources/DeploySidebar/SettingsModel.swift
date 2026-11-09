// SPDX-License-Identifier: GPL-3.0

import DeploySidebarCore
import Foundation
import Observation

/// Something fetched on demand.
enum Load<Value> {
    case notLoaded
    case loading
    case loaded(Value)
    case failed(String)
}

/// A metric being edited; the floor stays text until saving.
struct EditableMetric: Identifiable {
    let id = UUID()
    var label: String
    var expr: String
    var unit: MetricUnit
    var worse: Direction
    var floor: String
}

/// A service being edited. Identified by a UUID so test results survive reordering.
struct EditableService: Identifiable {
    let id = UUID()
    var name: String
    var renderServiceID: String
    var metrics: [EditableMetric]
}

/// The settings window's state: connections, timing, and the services and metrics to watch.
/// Saving writes the config file and Keychain entries, then tells the sidebar to reload.
@MainActor @Observable
final class SettingsModel {
    enum Status {
        case saved(String)
        case failed(String)
    }

    var grafanaURL: String
    var datasourceUID: String
    /// Text being typed into number fields, parsed on save.
    var windowMinutes: String
    var baselineMinutes: String
    var services: [EditableService]
    @ObservationIgnored private let renderPollSeconds: Int
    @ObservationIgnored private let metricsPollSeconds: Int

    var grafanaTokenInput = ""
    var hasGrafanaToken: Bool
    var datasources: Load<[Datasource]> = .notLoaded
    var grafanaTest: Load<Void> = .notLoaded

    var renderKeyInput = ""
    var hasRenderKey: Bool
    var renderServices: Load<[RenderService]> = .notLoaded

    var serviceTests: [UUID: Load<[SeriesResult]>] = [:]
    var status: Status?
    var openAtLogin = LoginItem.isEnabled

    @ObservationIgnored var onSave: (@MainActor () -> Void)?

    init() {
        let config: Config
        do {
            config = try Config.loadForEditing()
        } catch {
            config = (try? Config.parse(exampleConfig))
                ?? Config(grafanaURL: "", datasourceUID: "", services: [])
            status = .failed("\(error.localizedDescription). Showing the example config instead.")
        }
        grafanaURL = config.grafanaURL
        datasourceUID = config.datasourceUID
        windowMinutes = String(config.windowMinutes)
        baselineMinutes = String(config.baselineMinutes)
        renderPollSeconds = config.renderPollSeconds
        metricsPollSeconds = config.metricsPollSeconds
        services = config.services.map { service in
            EditableService(
                name: service.name,
                renderServiceID: service.renderServiceID,
                metrics: service.metrics.map { metric in
                    EditableMetric(
                        label: metric.label, expr: metric.expr, unit: metric.unit, worse: metric.worse,
                        floor: Self.floorText(metric.floor))
                }
            )
        }
        hasGrafanaToken = Keychain.grafanaToken() != nil
        hasRenderKey = Keychain.renderAPIKey() != nil
    }

    /// Fill the pickers straight away when the credentials are already there.
    func loadPickers() {
        loadDatasources()
        loadRenderServices()
    }

    func loadDatasources() {
        guard let grafana = grafana() else { return }
        datasources = .loading
        Task {
            do {
                let found = try await grafana.prometheusDatasources()
                // With a single Prometheus there is nothing to choose.
                if datasourceUID.isEmpty, found.count == 1 {
                    datasourceUID = found[0].uid
                }
                datasources = .loaded(found)
            } catch {
                datasources = .failed(error.localizedDescription)
            }
        }
    }

    func testGrafana() {
        guard let grafana = grafana() else {
            grafanaTest = .failed("Enter a URL and token first")
            return
        }
        grafanaTest = .loading
        Task {
            do {
                try await grafana.test()
                grafanaTest = .loaded(())
            } catch {
                grafanaTest = .failed(error.localizedDescription)
            }
        }
    }

    func loadRenderServices() {
        guard let key = renderKey() else {
            renderServices = .notLoaded
            return
        }
        renderServices = .loading
        Task {
            do {
                renderServices = .loaded(try await Render.services(apiKey: key))
            } catch {
                renderServices = .failed(error.localizedDescription)
            }
        }
    }

    /// Run a service's queries over the last 15 minutes and show each one's latest value.
    func testService(_ id: UUID) {
        guard let grafana = grafana() else {
            serviceTests[id] = .failed("Set up Grafana above first")
            return
        }
        guard let service = services.first(where: { $0.id == id }) else { return }
        let exprs = service.metrics.map(\.expr)
        serviceTests[id] = .loading
        Task {
            do {
                let to = Date()
                let results = try await grafana.queryRange(exprs, from: to.addingTimeInterval(-15 * 60), to: to, step: 60)
                serviceTests[id] = .loaded(results)
            } catch {
                serviceTests[id] = .failed(error.localizedDescription)
            }
        }
    }

    func addService() {
        services.append(EditableService(name: "", renderServiceID: "", metrics: []))
    }

    func removeService(_ id: UUID) {
        services.removeAll { $0.id == id }
        serviceTests[id] = nil
    }

    func addMetric(to serviceID: UUID) {
        guard let index = services.firstIndex(where: { $0.id == serviceID }) else { return }
        services[index].metrics.append(
            EditableMetric(label: "", expr: "", unit: .count, worse: .higher, floor: "0"))
        serviceTests[serviceID] = nil
    }

    func removeMetric(_ metricID: UUID, from serviceID: UUID) {
        guard let index = services.firstIndex(where: { $0.id == serviceID }) else { return }
        services[index].metrics.removeAll { $0.id == metricID }
        serviceTests[serviceID] = nil
    }

    func setOpenAtLogin(_ enabled: Bool) {
        do {
            try LoginItem.set(enabled)
        } catch {
            status = .failed("Couldn't change the login item: \(error.localizedDescription)")
        }
        openAtLogin = LoginItem.isEnabled
        if enabled, LoginItem.needsApproval {
            status = .failed("Allow Deploy Sidebar in System Settings → General → Login Items")
        }
    }

    /// Parse the number fields, store any new credentials, and write the config.
    func save() {
        do {
            let config = try buildConfig()
            let token = grafanaTokenInput.trimmingCharacters(in: .whitespacesAndNewlines)
            if !token.isEmpty {
                try Keychain.storeGrafanaToken(token)
                grafanaTokenInput = ""
                hasGrafanaToken = true
            }
            let key = renderKeyInput.trimmingCharacters(in: .whitespacesAndNewlines)
            if !key.isEmpty {
                try Keychain.storeRenderAPIKey(key)
                renderKeyInput = ""
                hasRenderKey = true
            }
            try config.save()
            grafanaURL = config.grafanaURL
            onSave?()
            if let problem = config.validationProblem() {
                status = .saved("Saved, but the sidebar won't run yet: \(problem)")
            } else {
                status = .saved("Saved")
            }
        } catch {
            status = .failed(error.localizedDescription)
        }
    }

    private func buildConfig() throws -> Config {
        var url = grafanaURL.trimmingCharacters(in: .whitespacesAndNewlines)
        while url.hasSuffix("/") { url.removeLast() }
        return Config(
            grafanaURL: url,
            datasourceUID: datasourceUID.trimmingCharacters(in: .whitespacesAndNewlines),
            windowMinutes: try Self.parseMinutes(windowMinutes, field: "Minutes to show"),
            baselineMinutes: try Self.parseMinutes(baselineMinutes, field: "Baseline minutes"),
            renderPollSeconds: renderPollSeconds,
            metricsPollSeconds: metricsPollSeconds,
            services: try services.map { service in
                Service(
                    name: service.name,
                    renderServiceID: service.renderServiceID.trimmingCharacters(in: .whitespacesAndNewlines),
                    metrics: try service.metrics.map { metric in
                        let text = metric.floor.trimmingCharacters(in: .whitespaces)
                        guard let floor = text.isEmpty ? 0 : Double(text) else {
                            throw AppError("The floor for \(metric.label) isn't a number")
                        }
                        return Metric(label: metric.label, expr: metric.expr, unit: metric.unit, worse: metric.worse, floor: floor)
                    }
                )
            }
        )
    }

    /// A client from what is on screen: the typed token, else the stored one.
    private func grafana() -> Grafana? {
        let url = grafanaURL.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !url.isEmpty else { return nil }
        let typed = grafanaTokenInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let token = typed.isEmpty ? Keychain.grafanaToken() : typed else { return nil }
        return Grafana(url: url, token: token, datasourceUID: datasourceUID.trimmingCharacters(in: .whitespaces))
    }

    private func renderKey() -> String? {
        let typed = renderKeyInput.trimmingCharacters(in: .whitespacesAndNewlines)
        return typed.isEmpty ? Keychain.renderAPIKey() : typed
    }

    private static func parseMinutes(_ text: String, field: String) throws -> Int {
        guard let minutes = Int(text.trimmingCharacters(in: .whitespaces)), minutes > 0 else {
            throw AppError("\(field) must be a whole number above zero")
        }
        return minutes
    }

    /// A floor as typed text: `5` rather than `5.0` for whole numbers.
    private static func floorText(_ floor: Double) -> String {
        floor == floor.rounded() && abs(floor) < 1e15 ? String(Int(floor)) : String(floor)
    }
}
