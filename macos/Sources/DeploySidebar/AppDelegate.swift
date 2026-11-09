// SPDX-License-Identifier: GPL-3.0

import AppKit
import DeploySidebarCore
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    let model = SidebarModel(mode: LaunchOptions.current.mode)
    private let options = LaunchOptions.current
    private var panel: SidebarPanel?
    private var settingsWindow: NSWindow?

    func applicationWillFinishLaunching(_ notification: Notification) {
        // Info.plist sets LSUIElement, but `swift run` has no Info.plist.
        NSApp.setActivationPolicy(options.settingsOnly ? .regular : .accessory)
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        installEditMenuIfNeeded()
        if options.settingsOnly {
            showSettings()
            return
        }
        let panel = SidebarPanel(model: model, openSettings: { [weak self] in self?.showSettings() })
        self.panel = panel
        model.onSurfaceChange = { [weak panel] in panel?.sync() }
        model.start()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        options.settingsOnly
    }

    /// Open the settings window, or bring it forward when it is already open.
    func showSettings() {
        if let settingsWindow, settingsWindow.isVisible {
            NSApp.activate()
            settingsWindow.makeKeyAndOrderFront(nil)
            return
        }
        let settings = SettingsModel()
        if !options.settingsOnly {
            settings.onSave = { [weak self] in self?.model.reloadIfChanged(force: true) }
        }
        let window = NSWindow(contentViewController: NSHostingController(rootView: SettingsView(model: settings)))
        window.title = "Deploy Sidebar Settings"
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable]
        window.setContentSize(NSSize(width: 760, height: 860))
        window.contentMinSize = NSSize(width: 600, height: 400)
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.center()
        window.setFrameAutosaveName("DeploySidebarSettings")
        settingsWindow = window
        if options.settingsOnly {
            // Launched from a terminal, which never yields activation, so insist.
            NSApp.activate(ignoringOtherApps: true)
        } else {
            NSApp.activate()
        }
        window.makeKeyAndOrderFront(nil)
    }

    func windowWillClose(_ notification: Notification) {
        if (notification.object as? NSWindow) === settingsWindow {
            settingsWindow = nil
        }
    }

    /// Without an Edit menu, Cmd-V does nothing in text fields, which makes pasting tokens impossible.
    /// A menu bar app never shows its main menu, but its key equivalents still work.
    private func installEditMenuIfNeeded() {
        let mainMenu = NSApp.mainMenu ?? NSMenu()
        let hasPaste = mainMenu.items.contains { item in
            item.submenu?.items.contains { $0.action == #selector(NSText.paste(_:)) } == true
        }
        guard !hasPaste else { return }

        let edit = NSMenu(title: "Edit")
        edit.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
        let redo = edit.addItem(withTitle: "Redo", action: Selector(("redo:")), keyEquivalent: "z")
        redo.keyEquivalentModifierMask = [.command, .shift]
        edit.addItem(.separator())
        edit.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        edit.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        edit.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        let item = NSMenuItem(title: "Edit", action: nil, keyEquivalent: "")
        item.submenu = edit
        mainMenu.addItem(item)
        NSApp.mainMenu = mainMenu
    }
}
