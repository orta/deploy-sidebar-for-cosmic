// SPDX-License-Identifier: GPL-3.0

import AppKit
import DeploySidebarCore
import SwiftUI

/// The sidebar's window: a borderless panel floating at the top right of the screen the pointer
/// is on, which never takes focus. The macOS stand-in for the COSMIC layer surface.
@MainActor
final class SidebarPanel {
    private static let margin: CGFloat = 12

    private let model: SidebarModel
    private let panel: NSPanel
    private let host: SidebarHostingView
    /// The screen the sidebar opened on, kept until it closes so it doesn't jump between screens.
    private var screen: NSScreen?
    private var fadingOut = false

    init(model: SidebarModel, openSettings: @escaping @MainActor () -> Void) {
        self.model = model
        panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: SidebarView.width, height: 100),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: true
        )
        panel.isFloatingPanel = true
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.hidesOnDeactivate = false
        panel.becomesKeyOnlyIfNeeded = true
        panel.isMovable = false
        panel.isReleasedWhenClosed = false
        panel.animationBehavior = .none

        host = SidebarHostingView(rootView: SidebarView(model: model, openSettings: openSettings, onResize: { _ in }))
        host.rootView = SidebarView(model: model, openSettings: openSettings) { [weak self] size in
            self?.place(size)
        }
        panel.contentView = host

        NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                if let screen = self.screen, !NSScreen.screens.contains(screen) {
                    self.screen = nil
                }
                self.place(self.panel.frame.size)
            }
        }
    }

    /// Show, fade or hide the panel to match the model.
    func sync() {
        guard model.isShowing else {
            if panel.isVisible {
                panel.orderOut(nil)
                fadingOut = false
                screen = nil
            }
            return
        }
        if !panel.isVisible {
            screen = Self.screenUnderPointer()
            fadingOut = false
            place(host.fittingSize)
            panel.alphaValue = 0
            panel.orderFrontRegardless()
            fade(to: 1, over: SidebarModel.fadeIn)
        } else if model.fadeStarted != nil, !fadingOut {
            fadingOut = true
            fade(to: 0, over: SidebarModel.fadeOut)
        } else if model.fadeStarted == nil, fadingOut {
            // A new deploy arrived mid-fade.
            fadingOut = false
            fade(to: 1, over: SidebarModel.fadeIn)
        }
    }

    /// Size the panel to its content, keeping it pinned to the screen's top right corner.
    private func place(_ size: CGSize) {
        guard size.width > 0, size.height > 0, let screen = screen ?? NSScreen.main else { return }
        let visible = screen.visibleFrame
        let frame = NSRect(
            x: visible.maxX - size.width - Self.margin,
            y: visible.maxY - size.height - Self.margin,
            width: size.width,
            height: size.height
        ).integral
        guard frame != panel.frame else { return }
        panel.setFrame(frame, display: true)
        panel.invalidateShadow()
    }

    private func fade(to alpha: CGFloat, over duration: TimeInterval) {
        let panel = panel
        NSAnimationContext.runAnimationGroup { context in
            context.duration = duration
            panel.animator().alphaValue = alpha
        }
    }

    private static func screenUnderPointer() -> NSScreen? {
        let pointer = NSEvent.mouseLocation
        return NSScreen.screens.first { NSMouseInRect(pointer, $0.frame, false) } ?? NSScreen.main
    }
}

/// Takes clicks without the panel having to become key first, so links work on the first click.
final class SidebarHostingView: NSHostingView<SidebarView> {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool {
        true
    }
}
