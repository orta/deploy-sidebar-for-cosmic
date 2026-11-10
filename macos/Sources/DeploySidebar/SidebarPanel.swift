// SPDX-License-Identifier: GPL-3.0

import AppKit
import DeploySidebarCore
import SwiftUI

/// The sidebar's window: a borderless panel floating in a corner of a screen, which never takes
/// focus. The macOS stand-in for the COSMIC layer surface. Dragging it moves it with the pointer,
/// and letting go snaps it to the nearest corner of that screen, which is remembered.
@MainActor
final class SidebarPanel {
    private static let margin: CGFloat = 12
    /// How far the pointer moves with the button down before it counts as a drag.
    private static let dragThreshold: CGFloat = 6

    /// A press on the panel, which becomes a drag once it moves far enough.
    private struct Drag {
        let start: NSPoint
        let origin: NSPoint
        var moving = false
    }

    /// What the event monitor does with a mouse event.
    private enum Outcome: Sendable {
        case pass
        case swallow
        /// Deliver the release far outside the panel, so the button it went down on doesn't fire.
        case releaseOutside
    }

    private let model: SidebarModel
    private let panel: NSPanel
    private let host: SidebarHostingView
    /// The screen the sidebar opened on, kept until it closes so it doesn't jump between screens.
    private var screen: NSScreen?
    private var fadingOut = false
    private var placement = Placement.load()
    private var drag: Drag?

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

        _ = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .leftMouseDragged, .leftMouseUp]) {
            [weak self] event in
            // Local monitors run on the main thread, with the event the app is about to dispatch.
            nonisolated(unsafe) let event = event
            let outcome = MainActor.assumeIsolated { self?.handle(event) ?? .pass }
            switch outcome {
            case .pass:
                return event
            case .swallow:
                return nil
            case .releaseOutside:
                return NSEvent.mouseEvent(
                    with: .leftMouseUp, location: NSPoint(x: -10_000, y: -10_000),
                    modifierFlags: event.modifierFlags, timestamp: event.timestamp,
                    windowNumber: event.windowNumber, context: nil, eventNumber: event.eventNumber,
                    clickCount: event.clickCount, pressure: 0
                )
            }
        }

        NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                if let screen = self.screen, !NSScreen.screens.contains(screen) {
                    self.screen = self.savedScreen()
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
            screen = savedScreen() ?? Self.screenUnderPointer()
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

    /// Size the panel to its content, keeping it pinned to the saved corner of its screen.
    private func place(_ size: CGSize, animate: Bool = false) {
        // The pointer owns the position mid-drag.
        guard drag?.moving != true else { return }
        guard size.width > 0, size.height > 0, let screen = screen ?? NSScreen.main else { return }
        let visible = screen.visibleFrame
        let corner = placement.corner
        let frame = NSRect(
            x: corner.isLeft ? visible.minX + Self.margin : visible.maxX - size.width - Self.margin,
            y: corner.isTop ? visible.maxY - size.height - Self.margin : visible.minY + Self.margin,
            width: size.width,
            height: size.height
        ).integral
        guard frame != panel.frame else { return }
        panel.setFrame(frame, display: true, animate: animate)
        panel.invalidateShadow()
    }

    /// Move the panel with a drag, and snap it to a corner when it is let go.
    private func handle(_ event: NSEvent) -> Outcome {
        guard event.window === panel else { return .pass }
        let pointer = NSEvent.mouseLocation
        switch event.type {
        case .leftMouseDown:
            drag = Drag(start: pointer, origin: panel.frame.origin)
            return .pass
        case .leftMouseDragged:
            guard var drag else { return .pass }
            let dx = pointer.x - drag.start.x
            let dy = pointer.y - drag.start.y
            if !drag.moving {
                guard hypot(dx, dy) >= Self.dragThreshold else { return .pass }
                drag.moving = true
                self.drag = drag
            }
            panel.setFrameOrigin(NSPoint(x: drag.origin.x + dx, y: drag.origin.y + dy))
            return .swallow
        case .leftMouseUp:
            guard let drag else { return .pass }
            self.drag = nil
            guard drag.moving else { return .pass }
            drop(at: pointer)
            return .releaseOutside
        default:
            return .pass
        }
    }

    /// Snap to the corner of the screen under the pointer nearest to it, and remember it.
    private func drop(at pointer: NSPoint) {
        guard let target = NSScreen.screens.first(where: { NSMouseInRect(pointer, $0.frame, false) })
            ?? screen ?? NSScreen.main
        else { return }
        let frame = target.frame
        // Screen coordinates grow upwards; corners are worked out with y growing downwards.
        let corner = Corner.nearest(
            x: pointer.x - frame.minX, y: frame.maxY - pointer.y, width: frame.width, height: frame.height
        )
        placement = Placement(corner: corner, output: target.localizedName)
        do {
            try placement.save()
        } catch {
            NSLog("Saving the sidebar position: %@", error.localizedDescription)
        }
        screen = target
        place(panel.frame.size, animate: true)
    }

    /// The screen the sidebar was last dropped on, when it is connected.
    private func savedScreen() -> NSScreen? {
        guard let name = placement.output else { return nil }
        return NSScreen.screens.first { $0.localizedName == name }
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
