// swift-tools-version: 6.0
// SPDX-License-Identifier: GPL-3.0

import PackageDescription

let package = Package(
    name: "DeploySidebar",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "DeploySidebar", targets: ["DeploySidebar"]),
    ],
    targets: [
        // Config, TOML, API clients, health checks and the deploy state machine; no UI.
        .target(name: "DeploySidebarCore"),
        // The menu bar app, the floating sidebar panel, the settings window and the CLI.
        .executableTarget(name: "DeploySidebar", dependencies: ["DeploySidebarCore"]),
        .testTarget(name: "DeploySidebarCoreTests", dependencies: ["DeploySidebarCore"]),
    ]
)
