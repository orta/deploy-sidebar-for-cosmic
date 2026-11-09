// SPDX-License-Identifier: GPL-3.0

import Foundation
import Security

/// Credentials, kept in the login Keychain.
public enum Keychain {
    static let service = "io.orta.DeploySidebar"
    static let grafanaTokenAccount = "grafana_token"
    static let renderAPIKeyAccount = "render_api_key"

    /// The RenderVercelDeploy menu bar app's entry, read so the Render key only has to be set up once.
    static let siblingService = "io.orta.RenderVercelDeploy"

    /// Store the Grafana service account token.
    public static func storeGrafanaToken(_ token: String) throws {
        try store(token, account: grafanaTokenAccount)
    }

    /// Store a Render API key for this app, overriding RenderVercelDeploy's.
    public static func storeRenderAPIKey(_ key: String) throws {
        try store(key, account: renderAPIKeyAccount)
    }

    /// The Grafana token, from the Keychain or `GRAFANA_TOKEN`.
    public static func grafanaToken() -> String? {
        read(service: service, account: grafanaTokenAccount) ?? environment("GRAFANA_TOKEN")
    }

    /// The Render key: this app's own, then RenderVercelDeploy's, then `RENDER_API_KEY`.
    public static func renderAPIKey() -> String? {
        read(service: service, account: renderAPIKeyAccount)
            ?? read(service: siblingService, account: renderAPIKeyAccount)
            ?? environment("RENDER_API_KEY")
    }

    private static func environment(_ name: String) -> String? {
        ProcessInfo.processInfo.environment[name].flatMap { $0.isEmpty ? nil : $0 }
    }

    private static func read(service: String, account: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data,
              let value = String(data: data, encoding: .utf8),
              !value.isEmpty
        else { return nil }
        return value
    }

    private static func store(_ value: String, account: String) throws {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        let data = Data(value.utf8)
        var status = SecItemUpdate(query as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            var item = query
            item[kSecValueData as String] = data
            item[kSecAttrLabel as String] = "Deploy Sidebar \(account)"
            status = SecItemAdd(item as CFDictionary, nil)
        }
        guard status == errSecSuccess else {
            let reason = SecCopyErrorMessageString(status, nil) as String? ?? "error \(status)"
            throw AppError("Could not save to the Keychain: \(reason)")
        }
    }
}
