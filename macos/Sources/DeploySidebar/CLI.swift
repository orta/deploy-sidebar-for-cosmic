// SPDX-License-Identifier: GPL-3.0

import DeploySidebarCore
import Foundation

/// Setup subcommands, run instead of the app.
enum CLI {
    static let help = """
        DeploySidebar

        Usage:
          DeploySidebar                      Run the sidebar (idle until a deploy)
          DeploySidebar --preview <service>  Show the sidebar now for a service's latest deploy
          DeploySidebar --demo               Show the sidebar with synthetic data, no network
          DeploySidebar settings             Open only the settings window
          DeploySidebar init-config          Write the example config if there is none
          DeploySidebar set-grafana-token    Store a Grafana service account token (reads stdin)
          DeploySidebar set-render-key       Store a Render API key (reads stdin)
          DeploySidebar list-services        List Render services and their IDs
          DeploySidebar check                Run every configured query and print the results

        """

    /// Run a subcommand, returning the process exit code.
    static func run(_ command: String) -> Int32 {
        do {
            switch command {
            case "init-config":
                try initConfig()
            case "set-grafana-token":
                try Keychain.storeGrafanaToken(try readSecret("Grafana token"))
                printError("Stored the Grafana token in the Keychain.")
            case "set-render-key":
                try Keychain.storeRenderAPIKey(try readSecret("Render API key"))
                printError("Stored the Render API key in the Keychain.")
            case "list-services":
                try blockOn(listServices)
            case "check":
                try blockOn(check)
            case "help", "--help", "-h":
                print(help, terminator: "")
            default:
                throw AppError("Unknown command `\(command)`\n\n\(help)")
            }
            return 0
        } catch {
            printError(error.localizedDescription)
            return 1
        }
    }

    static func printError(_ message: String) {
        FileHandle.standardError.write(Data((message + "\n").utf8))
    }

    private static func initConfig() throws {
        let path = Config.path
        if FileManager.default.fileExists(atPath: path.path) {
            throw AppError("\(Config.displayPath) already exists")
        }
        try FileManager.default.createDirectory(at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
        try exampleConfig.write(to: path, atomically: true, encoding: .utf8)
        print("Wrote \(Config.displayPath)")
    }

    private static func readSecret(_ name: String) throws -> String {
        printError("Paste the \(name) and press enter:")
        let secret = (readLine() ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if secret.isEmpty {
            throw AppError("No \(name) given")
        }
        return secret
    }

    @Sendable private static func listServices() async throws {
        guard let key = Keychain.renderAPIKey() else { throw AppError("No Render API key, run set-render-key") }
        for service in try await Render.services(apiKey: key) {
            print("\(service.id)  \(service.name)")
        }
    }

    /// Query the last 15 minutes of every metric, to confirm the config and token work.
    @Sendable private static func check() async throws {
        let config = try Config.load()
        guard let token = Keychain.grafanaToken() else { throw AppError("No Grafana token, run set-grafana-token") }
        let grafana = Grafana(url: config.grafanaURL, token: token, datasourceUID: config.datasourceUID)

        let to = Date()
        let from = to.addingTimeInterval(-15 * 60)
        for service in config.services {
            print(service.name)
            let results = try await grafana.queryRange(service.metrics.map(\.expr), from: from, to: to, step: 60)
            for (metric, result) in zip(service.metrics, results) {
                let line = switch result {
                case .success(let points):
                    points.last.map { "\(Health.format(metric.unit, $0.value)) (\(points.count) points)" } ?? "no data"
                case .failure(let error):
                    "error: \(error.message)"
                }
                let padding = String(repeating: " ", count: max(0, 20 - metric.label.count))
                print("  \(metric.label)\(padding) \(line)")
            }
        }
    }

    /// Run async work from the synchronous entry point, before any run loop exists.
    private static func blockOn(_ work: @escaping @Sendable () async throws -> Void) throws {
        final class Outcome: @unchecked Sendable {
            var error: Error?
        }
        let outcome = Outcome()
        let done = DispatchSemaphore(value: 0)
        Task.detached {
            do {
                try await work()
            } catch {
                outcome.error = error
            }
            done.signal()
        }
        done.wait()
        if let error = outcome.error { throw error }
    }
}
