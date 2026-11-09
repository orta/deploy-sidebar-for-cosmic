// SPDX-License-Identifier: GPL-3.0

import DeploySidebarCore
import SwiftUI

private let renderAPIKeysURL = URL(string: "https://dashboard.render.com/u/settings#api-keys")!

/// The settings window: connections, timing, and the services and metrics to watch.
struct SettingsView: View {
    @Bindable var model: SettingsModel

    var body: some View {
        Form {
            Section("General") {
                Toggle("Open at login", isOn: Binding(get: { model.openAtLogin }, set: { model.setOpenAtLogin($0) }))
            }
            grafanaSection
            renderSection
            timingSection
            ForEach($model.services) { $service in
                ServiceSection(model: model, service: $service)
            }
            Section {
                Button("Add Service", systemImage: "plus", action: model.addService)
            }
        }
        .formStyle(.grouped)
        .safeAreaInset(edge: .bottom, spacing: 0) { footer }
        .frame(minWidth: 600, minHeight: 400)
        .task { model.loadPickers() }
        .onChange(of: model.grafanaURL) { model.grafanaTest = .notLoaded }
        .onChange(of: model.grafanaTokenInput) { model.grafanaTest = .notLoaded }
        .onChange(of: model.datasourceUID) { model.grafanaTest = .notLoaded }
    }

    private var grafanaSection: some View {
        Section {
            TextField("URL", text: $model.grafanaURL, prompt: Text("https://grafana.example.com"))
            SecureField(
                "Token", text: $model.grafanaTokenInput,
                prompt: Text(model.hasGrafanaToken ? "Stored in the Keychain, type to replace" : "glsa_…"))
            let url = model.grafanaURL.trimmingCharacters(in: CharacterSet(charactersIn: "/ "))
            if !url.isEmpty, let serviceAccounts = URL(string: "\(url)/org/serviceaccounts") {
                Link("Create a token in Grafana", destination: serviceAccounts)
            }
            LabeledContent("Prometheus datasource") {
                HStack {
                    datasourcePicker
                    Button("Reload Datasources", systemImage: "arrow.clockwise", action: model.loadDatasources)
                        .labelStyle(.iconOnly)
                        .buttonStyle(.borderless)
                }
            }
            if let note = datasourceNote {
                Text(note).foregroundStyle(.secondary)
            }
            HStack {
                Button("Test Connection", action: model.testGrafana)
                switch model.grafanaTest {
                case .notLoaded: EmptyView()
                case .loading: ProgressView().controlSize(.small)
                case .loaded: Label("Connected, queries work", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                case .failed(let error): Text(error).foregroundStyle(.red)
                }
            }
        } header: {
            Text("Grafana")
        } footer: {
            Text("A service account token with the Viewer role. Grafana's GitHub sign-in doesn't apply to tokens.")
                .sectionFooter()
        }
    }

    @ViewBuilder
    private var datasourcePicker: some View {
        if case .loaded(let datasources) = model.datasources, !datasources.isEmpty {
            Picker("Prometheus datasource", selection: $model.datasourceUID) {
                if !datasources.contains(where: { $0.uid == model.datasourceUID }) {
                    Text(model.datasourceUID.isEmpty ? "Choose…" : model.datasourceUID).tag(model.datasourceUID)
                }
                ForEach(datasources) { datasource in
                    Text("\(datasource.name) (\(datasource.uid))").tag(datasource.uid)
                }
            }
            .labelsHidden()
            .fixedSize()
        } else {
            TextField("Datasource UID", text: $model.datasourceUID, prompt: Text("Datasource UID"))
                .labelsHidden()
                .frame(width: 240)
        }
    }

    private var datasourceNote: String? {
        switch model.datasources {
        case .loading: "Loading datasources…"
        case .loaded(let datasources) where datasources.isEmpty: "No Prometheus datasources visible to this token"
        case .failed(let error): error
        case .notLoaded, .loaded: nil
        }
    }

    private var renderSection: some View {
        Section {
            SecureField(
                "API key", text: $model.renderKeyInput,
                prompt: Text(model.hasRenderKey ? "Stored key in use, type to replace" : "rnd_…"))
            HStack {
                Button("Load Services", action: model.loadRenderServices)
                switch model.renderServices {
                case .loading: ProgressView().controlSize(.small)
                case .failed(let error): Text(error).foregroundStyle(.red)
                case .notLoaded, .loaded: EmptyView()
                }
                Spacer()
                Link("Create a Render API key", destination: renderAPIKeysURL)
            }
        } header: {
            Text("Render")
        } footer: {
            Text("With no key here, the one saved by the RenderVercelDeploy menu bar app is used.")
                .sectionFooter()
        }
    }

    private var timingSection: some View {
        Section("Timing") {
            LabeledContent("Minutes to show after a deploy goes live") {
                TextField("Minutes to show", text: $model.windowMinutes, prompt: Text("10"))
                    .labelsHidden()
                    .frame(width: 80)
            }
            LabeledContent("Minutes before the deploy to compare against") {
                TextField("Baseline minutes", text: $model.baselineMinutes, prompt: Text("30"))
                    .labelsHidden()
                    .frame(width: 80)
            }
        }
    }

    private var footer: some View {
        VStack(spacing: 0) {
            Divider()
            HStack {
                switch model.status {
                case .saved(let message)?:
                    Text(message)
                case .failed(let message)?:
                    Text(message).foregroundStyle(.red)
                case nil:
                    Text("Saving writes the config file, and the sidebar reloads it straight away.")
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button("Save", action: model.save)
                    .keyboardShortcut("s")
                    .buttonStyle(.borderedProminent)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
        }
        .background(.bar)
    }
}

/// One service: its name, its Render service, and its metrics.
private struct ServiceSection: View {
    @Bindable var model: SettingsModel
    @Binding var service: EditableService

    var body: some View {
        Section(service.name.isEmpty ? "New service" : service.name) {
            TextField("Name", text: $service.name, prompt: Text("api"))
            renderPicker
            ForEach($service.metrics) { $metric in
                MetricEditor(metric: $metric, result: result(for: metric)) {
                    model.removeMetric(metric.id, from: service.id)
                }
            }
            HStack {
                Button("Add Metric", systemImage: "plus") { model.addMetric(to: service.id) }
                Button("Test Queries") { model.testService(service.id) }
                switch model.serviceTests[service.id] {
                case .loading?: ProgressView().controlSize(.small)
                case .failed(let error)?: Text(error).foregroundStyle(.red)
                default: EmptyView()
                }
                Spacer()
                Button("Remove Service", role: .destructive) { model.removeService(service.id) }
            }
        }
        .onChange(of: service.metrics.map(\.expr)) { model.serviceTests[service.id] = nil }
    }

    @ViewBuilder
    private var renderPicker: some View {
        if case .loaded(let available) = model.renderServices {
            Picker("Render service", selection: $service.renderServiceID) {
                if !available.contains(where: { $0.id == service.renderServiceID }) {
                    Text(service.renderServiceID.isEmpty ? "Choose…" : service.renderServiceID)
                        .tag(service.renderServiceID)
                }
                ForEach(available) { renderService in
                    Text(renderService.name).tag(renderService.id)
                }
            }
            .onChange(of: service.renderServiceID) { _, id in
                if service.name.isEmpty, let picked = available.first(where: { $0.id == id }) {
                    service.name = picked.name
                }
            }
        } else {
            TextField("Render service ID", text: $service.renderServiceID, prompt: Text("srv-…"))
        }
    }

    private func result(for metric: EditableMetric) -> String? {
        guard case .loaded(let results)? = model.serviceTests[service.id],
              let index = service.metrics.firstIndex(where: { $0.id == metric.id })
        else { return nil }
        guard results.indices.contains(index) else { return "No result" }
        switch results[index] {
        case .success(let points):
            return points.last.map { "Now: \(Health.format(metric.unit, $0.value))" } ?? "No data"
        case .failure(let error):
            return "Error: \(error.message)"
        }
    }
}

/// A metric's label, unit, direction, floor and query.
private struct MetricEditor: View {
    @Binding var metric: EditableMetric
    let result: String?
    let remove: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                TextField("Label", text: $metric.label, prompt: Text("Label"))
                    .labelsHidden()
                    .textFieldStyle(.roundedBorder)
                Picker("Unit", selection: $metric.unit) {
                    ForEach(MetricUnit.allCases, id: \.self) { Text($0.label).tag($0) }
                }
                .labelsHidden()
                .fixedSize()
                Picker("Direction", selection: $metric.worse) {
                    ForEach(Direction.allCases, id: \.self) { Text($0.label).tag($0) }
                }
                .labelsHidden()
                .fixedSize()
                Text("Floor").foregroundStyle(.secondary)
                TextField("Floor", text: $metric.floor, prompt: Text("0"))
                    .labelsHidden()
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 90)
                Button("Remove Metric", systemImage: "trash", role: .destructive, action: remove)
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
            }
            TextField("Query", text: $metric.expr, prompt: Text("PromQL, e.g. sum(rate(…[2m]))"), axis: .vertical)
                .labelsHidden()
                .textFieldStyle(.roundedBorder)
                .font(.system(.callout, design: .monospaced))
                .lineLimit(1...5)
            if let result {
                Text(result)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
        }
        .padding(.vertical, 4)
    }
}

extension Text {
    /// Explanatory text under a section, aligned with the rows above it.
    fileprivate func sectionFooter() -> some View {
        foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}
