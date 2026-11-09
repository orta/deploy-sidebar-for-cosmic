// SPDX-License-Identifier: GPL-3.0

import AppKit
import DeploySidebarCore
import SwiftUI

/// What the command line asked for, read by the app delegate once the app launches.
struct LaunchOptions {
    var mode = LaunchMode.normal
    /// Only the settings window, like `cosmic-deploy-sidebar settings`: no sidebar, and closing
    /// the window quits.
    var settingsOnly = false

    nonisolated(unsafe) static var current = LaunchOptions()
}

@main
enum Main {
    /// Run a setup subcommand, or launch the app.
    static func main() {
        // Very old macOS passes a process serial number to apps opened from Finder.
        let args = CommandLine.arguments.dropFirst().filter { !$0.hasPrefix("-psn_") }
        var options = LaunchOptions()
        switch args.first {
        case nil:
            break
        case "--demo":
            options.mode = .demo
        case "--preview":
            guard args.count > 1 else {
                CLI.printError("Usage: DeploySidebar --preview <service name>")
                exit(2)
            }
            options.mode = .preview(args[args.startIndex + 1])
        case "settings":
            options.settingsOnly = true
        case let command?:
            exit(CLI.run(command))
        }
        LaunchOptions.current = options
        DeploySidebarApp.main()
    }
}

struct DeploySidebarApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        MenuBarExtra(isInserted: .constant(!LaunchOptions.current.settingsOnly)) {
            MenuContent(model: delegate.model, showSettings: { delegate.showSettings() })
        } label: {
            Image(systemName: delegate.model.isShowing ? "waveform.path.ecg.rectangle.fill" : "waveform.path.ecg")
        }
    }
}

/// The menu bar icon's menu.
struct MenuContent: View {
    let model: SidebarModel
    let showSettings: () -> Void

    var body: some View {
        Text(summary)
        Divider()
        if !model.isDemo, let services = model.config?.services, !services.isEmpty {
            Menu("Preview Latest Deploy") {
                ForEach(services.map(\.name), id: \.self) { name in
                    Button(name) { model.previewLatest(name) }
                }
            }
        }
        Button("Settings…", action: showSettings)
            .keyboardShortcut(",")
        Divider()
        Button("Quit Deploy Sidebar") { NSApp.terminate(nil) }
            .keyboardShortcut("q")
    }

    private var summary: String {
        if let notice = model.notice {
            return truncated(notice)
        }
        if !model.watches.isEmpty {
            return model.watches.map { "\(model.serviceName($0.service)): \(model.status($0).text)" }
                .joined(separator: ", ")
        }
        guard let services = model.config?.services, !services.isEmpty else { return "No services configured" }
        return truncated("Watching " + services.map(\.name).joined(separator: ", "))
    }

    private func truncated(_ text: String) -> String {
        text.count <= 70 ? text : text.prefix(69) + "…"
    }
}
