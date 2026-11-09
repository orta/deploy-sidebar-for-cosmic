// SPDX-License-Identifier: GPL-3.0

import ServiceManagement

/// Starting at login, the macOS counterpart of the systemd user service on Linux.
enum LoginItem {
    @MainActor static var isEnabled: Bool {
        SMAppService.mainApp.status == .enabled
    }

    /// Whether macOS is waiting for the user to allow the login item in System Settings.
    @MainActor static var needsApproval: Bool {
        SMAppService.mainApp.status == .requiresApproval
    }

    /// Register or unregister the app as a login item.
    @MainActor static func set(_ enabled: Bool) throws {
        if enabled {
            try SMAppService.mainApp.register()
        } else {
            try SMAppService.mainApp.unregister()
        }
    }
}
