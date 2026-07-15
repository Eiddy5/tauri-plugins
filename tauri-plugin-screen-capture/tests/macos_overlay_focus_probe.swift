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
    panel: CGWindowID
) -> [CGWindowID]? {
    guard let targetIndex = ids.firstIndex(of: target) else {
        return nil
    }
    guard let spanStart = ids.firstIndex(of: panel), spanStart < targetIndex else {
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

let panel = NSPanel(
    contentRect: target.frame,
    styleMask: [.borderless, .nonactivatingPanel],
    backing: .buffered,
    defer: false
)
panel.level = target.level
panel.isOpaque = false
panel.hasShadow = false
panel.isFloatingPanel = false
panel.hidesOnDeactivate = false
panel.backgroundColor = .clear
panel.ignoresMouseEvents = true
panel.collectionBehavior = [
    .canJoinAllSpaces,
    .fullScreenAuxiliary,
    .stationary,
]
precondition(panel.collectionBehavior.contains(.stationary))
precondition(!panel.collectionBehavior.contains(.transient))

let overlayView = NSView(frame: NSRect(origin: .zero, size: target.frame.size))
overlayView.wantsLayer = true
let rootLayer = CALayer()
rootLayer.frame = overlayView.bounds
overlayView.layer = rootLayer
panel.contentView = overlayView

let markerLength: CGFloat = 32
let markerThickness: CGFloat = 4
let width = target.frame.width
let height = target.frame.height
let cornerFrames = [
    CGRect(x: 0, y: height - markerLength, width: markerLength, height: markerLength),
    CGRect(x: width - markerLength, y: height - markerLength, width: markerLength, height: markerLength),
    CGRect(x: 0, y: 0, width: markerLength, height: markerLength),
    CGRect(x: width - markerLength, y: 0, width: markerLength, height: markerLength),
]
for (index, corner) in cornerFrames.enumerated() {
    let horizontal = CALayer()
    horizontal.backgroundColor = NSColor.systemGreen.cgColor
    horizontal.frame = CGRect(
        x: corner.minX,
        y: index < 2 ? corner.maxY - markerThickness : corner.minY,
        width: markerLength,
        height: markerThickness
    )
    rootLayer.addSublayer(horizontal)

    let vertical = CALayer()
    vertical.backgroundColor = NSColor.systemGreen.cgColor
    vertical.frame = CGRect(
        x: index % 2 == 0 ? corner.minX : corner.maxX - markerThickness,
        y: corner.minY,
        width: markerThickness,
        height: markerLength
    )
    rootLayer.addSublayer(vertical)
}
panel.order(.above, relativeTo: target.windowNumber)

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
let panelID = CGWindowID(panel.windowNumber)
let initialIDs = visibleWindowIDs()
guard let cachedSpan = orderSpan(initialIDs, target: targetID, panel: panelID) else {
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

panel.order(.above, relativeTo: target.windowNumber)
sibling.order(.above, relativeTo: target.windowNumber)
advanceWindowServer()
let repairedIDs = visibleWindowIDs()
guard let repairedSpan = orderSpan(repairedIDs, target: targetID, panel: panelID) else {
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

panel.close()
sibling.close()
target.close()
print("PASS: single overlay panel focus reorder reproduced, repaired, and stable with sibling")
