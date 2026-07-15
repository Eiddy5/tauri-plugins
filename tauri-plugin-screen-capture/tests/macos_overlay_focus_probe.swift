import AppKit
import CoreGraphics
import Foundation

@_silgen_name("CGWindowListCreate")
func rawCGWindowListCreate(
    _ option: CGWindowListOption,
    _ relativeToWindow: CGWindowID
) -> Unmanaged<CFArray>?

func visibleWindowIDs() -> [CGWindowID] {
    guard let list = rawCGWindowListCreate(
        .optionOnScreenOnly,
        kCGNullWindowID
    )?.takeRetainedValue() else {
        return []
    }
    return (0..<CFArrayGetCount(list)).compactMap { index in
        guard let raw = CFArrayGetValueAtIndex(list, index) else {
            return nil
        }
        return CGWindowID(UInt(bitPattern: raw))
    }
}

func orderSpan(
    _ ids: [CGWindowID],
    target: CGWindowID,
    panels: [CGWindowID]
) -> [CGWindowID]? {
    guard let targetIndex = ids.firstIndex(of: target) else {
        return nil
    }
    let panelIndexes = panels.compactMap { ids.firstIndex(of: $0) }
    guard panelIndexes.count == panels.count,
          let spanStart = panelIndexes.min(),
          spanStart < targetIndex else {
        return nil
    }
    return Array(ids[spanStart...targetIndex])
}

func orderMatches(
    _ ids: [CGWindowID],
    target: CGWindowID,
    expectedSpan: [CGWindowID]
) -> Bool {
    guard expectedSpan.last == target,
          let targetIndex = ids.firstIndex(of: target),
          targetIndex + 1 >= expectedSpan.count else {
        return false
    }
    let spanStart = targetIndex + 1 - expectedSpan.count
    return Array(ids[spanStart...targetIndex]) == expectedSpan
}

func advanceWindowServer() {
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)

let target = NSWindow(
    contentRect: NSRect(x: 120, y: 120, width: 480, height: 320),
    styleMask: [.titled, .closable, .resizable],
    backing: .buffered,
    defer: false
)
target.title = "macos-overlay-focus-probe-target"
target.level = .normal
target.orderFrontRegardless()

let panelFrames = [
    NSRect(x: 120, y: 408, width: 32, height: 32),
    NSRect(x: 568, y: 408, width: 32, height: 32),
    NSRect(x: 120, y: 120, width: 32, height: 32),
    NSRect(x: 568, y: 120, width: 32, height: 32),
]
let panels = panelFrames.map { frame -> NSPanel in
    let panel = NSPanel(
        contentRect: frame,
        styleMask: [.borderless, .nonactivatingPanel],
        backing: .buffered,
        defer: false
    )
    panel.level = target.level
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.ignoresMouseEvents = true
    panel.collectionBehavior = [
        .canJoinAllSpaces,
        .fullScreenAuxiliary,
        .transient,
        .ignoresCycle,
    ]
    precondition(panel.collectionBehavior.contains(.transient))
    precondition(panel.collectionBehavior.contains(.ignoresCycle))
    precondition(!panel.collectionBehavior.contains(.stationary))
    panel.order(.above, relativeTo: target.windowNumber)
    return panel
}

let sibling = NSWindow(
    contentRect: NSRect(x: 220, y: 220, width: 120, height: 80),
    styleMask: [.titled],
    backing: .buffered,
    defer: false
)
sibling.title = "macos-overlay-focus-probe-sibling"
sibling.level = target.level
sibling.order(.above, relativeTo: target.windowNumber)

advanceWindowServer()
let targetID = CGWindowID(target.windowNumber)
let panelIDs = panels.map { CGWindowID($0.windowNumber) }
let initialIDs = visibleWindowIDs()
guard let cachedSpan = orderSpan(initialIDs, target: targetID, panels: panelIDs) else {
    preconditionFailure("initial panel order was not established")
}
precondition(cachedSpan.contains(CGWindowID(sibling.windowNumber)))
precondition(orderMatches(initialIDs, target: targetID, expectedSpan: cachedSpan))

target.orderFrontRegardless()
advanceWindowServer()
precondition(
    !orderMatches(visibleWindowIDs(), target: targetID, expectedSpan: cachedSpan),
    "bringing the target forward did not reproduce panel order loss"
)

for panel in panels {
    panel.order(.above, relativeTo: target.windowNumber)
}
sibling.order(.above, relativeTo: target.windowNumber)
advanceWindowServer()
let repairedIDs = visibleWindowIDs()
guard let repairedSpan = orderSpan(repairedIDs, target: targetID, panels: panelIDs) else {
    preconditionFailure("relative panel reorder did not restore the invariant")
}
precondition(
    repairedSpan.contains(CGWindowID(sibling.windowNumber)),
    "relative panel reorder did not restore the invariant"
)
advanceWindowServer()
advanceWindowServer()
advanceWindowServer()
precondition(orderMatches(visibleWindowIDs(), target: targetID, expectedSpan: repairedSpan))

for panel in panels {
    panel.close()
}
sibling.close()
target.close()
print("PASS: focus reorder reproduced, repaired, and stable with sibling")
