#!/usr/bin/env swift

import AppKit
import CoreGraphics
import Darwin
import Foundation

let app = NSApplication.shared
app.setActivationPolicy(.accessory)

func windowRows() -> [[String: Any]] {
    CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
}

func number(_ row: [String: Any], _ key: CFString) -> Int? {
    row[key as String] as? Int
}

func bounds(_ row: [String: Any]) -> CGRect? {
    guard let dictionary = row[kCGWindowBounds as String] as? NSDictionary else {
        return nil
    }
    return CGRect(dictionaryRepresentation: dictionary as CFDictionary)
}

func intersects(_ first: CGRect, _ second: CGRect) -> Bool {
    first.width > 0 && first.height > 0
        && second.width > 0 && second.height > 0
        && first.minX < second.maxX
        && second.minX < first.maxX
        && first.minY < second.maxY
        && second.minY < first.maxY
}

func cornerFrames(_ target: CGRect) -> [CGRect] {
    let length = min(32.0, target.width / 2.0, target.height / 2.0)
    return [
        CGRect(x: target.minX, y: target.minY, width: length, height: length),
        CGRect(x: target.maxX - length, y: target.minY, width: length, height: length),
        CGRect(x: target.minX, y: target.maxY - length, width: length, height: length),
        CGRect(x: target.maxX - length, y: target.maxY - length, width: length, height: length),
    ]
}

func appKitRect(_ coreGraphicsRect: CGRect) -> NSRect {
    let mainDisplay = CGDisplayBounds(CGMainDisplayID())
    return NSRect(
        x: coreGraphicsRect.minX,
        y: mainDisplay.height - coreGraphicsRect.maxY,
        width: coreGraphicsRect.width,
        height: coreGraphicsRect.height
    )
}

let rowsBeforePlacement = windowRows()
let processID = getpid()
let requestedTargetID = ProcessInfo.processInfo.environment["TARGET_WINDOW_ID"].flatMap(Int.init)
let excludedOwners: Set<String> = [
    "Window Server", "Dock", "SystemUIServer", "控制中心", "通知中心", "ChatGPT",
]

guard let targetIndex = rowsBeforePlacement.firstIndex(where: { row in
    let windowID = number(row, kCGWindowNumber)
    let ownerPID = number(row, kCGWindowOwnerPID)
    let ownerName = row[kCGWindowOwnerName as String] as? String ?? ""
    let layer = number(row, kCGWindowLayer)
    let alpha = row[kCGWindowAlpha as String] as? Double ?? 0
    let frame = bounds(row)
    if let requestedTargetID {
        return windowID == requestedTargetID
    }
    return ownerPID != Int(processID)
        && !excludedOwners.contains(ownerName)
        && layer == 0
        && alpha > 0
        && frame.map { $0.width >= 64 && $0.height >= 64 } == true
}) else {
    fatalError("No real layer-0 target window is available for the probe")
}

let target = rowsBeforePlacement[targetIndex]
let targetID = number(target, kCGWindowNumber)!
let targetOwnerPID = number(target, kCGWindowOwnerPID)!
let targetOwnerName = target[kCGWindowOwnerName as String] as? String ?? "?"
let targetLayer = number(target, kCGWindowLayer)!
let targetFrame = bounds(target)!
let corners = cornerFrames(targetFrame)

let siblings = rowsBeforePlacement[..<targetIndex].filter { row in
    number(row, kCGWindowOwnerPID) == targetOwnerPID
        && number(row, kCGWindowLayer) == targetLayer
        && bounds(row) != nil
}
let siblingIDs = siblings.compactMap { number($0, kCGWindowNumber) }
let visibleCorners = corners.map { corner in
    siblings.allSatisfy { sibling in
        guard let siblingFrame = bounds(sibling) else { return false }
        return !intersects(siblingFrame, corner)
    }
}

var panels: [NSPanel] = []
for (index, corner) in corners.enumerated() {
    let panel = NSPanel(
        contentRect: appKitRect(corner),
        styleMask: [.borderless, .nonactivatingPanel],
        backing: .buffered,
        defer: false
    )
    panel.title = "TAURI_OVERLAY_ORDER_PROBE_\(index)"
    panel.backgroundColor = .green
    panel.level = NSWindow.Level(rawValue: targetLayer)
    panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
    panels.append(panel)
}
defer {
    for panel in panels {
        panel.orderOut(nil)
    }
}

let allPanelIDs = Set(panels.map(\.windowNumber))
let visiblePanelIDs = Set(panels.enumerated().compactMap { index, panel in
    visibleCorners[index] ? panel.windowNumber : nil
})
for (index, panel) in panels.enumerated() where visibleCorners[index] {
    panel.order(.above, relativeTo: targetID)
}

func placementIsValid() -> Bool {
    let rows = windowRows()
    guard let committedTargetIndex = rows.firstIndex(where: {
        number($0, kCGWindowNumber) == targetID
    }) else {
        return false
    }
    let committedPanelIDs = Set(rows.compactMap { row -> Int? in
        let windowID = number(row, kCGWindowNumber)
        return windowID.map(allPanelIDs.contains) == true ? windowID : nil
    })
    guard committedPanelIDs == visiblePanelIDs else {
        return false
    }

    for (cornerIndex, panel) in panels.enumerated() where visibleCorners[cornerIndex] {
        guard let panelIndex = rows.firstIndex(where: {
            number($0, kCGWindowNumber) == panel.windowNumber
        }), panelIndex < committedTargetIndex,
        number(rows[panelIndex], kCGWindowLayer) == targetLayer else {
            return false
        }
        for row in rows[(panelIndex + 1) ..< committedTargetIndex] {
            let windowID = number(row, kCGWindowNumber)
            if windowID.map(visiblePanelIDs.contains) == true {
                continue
            }
            guard number(row, kCGWindowOwnerPID) == targetOwnerPID,
                  number(row, kCGWindowLayer) == targetLayer,
                  let frame = bounds(row),
                  !intersects(frame, corners[cornerIndex]) else {
                return false
            }
        }
    }
    return true
}

var timerValid: Bool?
_ = Timer.scheduledTimer(withTimeInterval: 0.0, repeats: false) { _ in
    timerValid = placementIsValid()
}

let deadline = Date(timeIntervalSinceNow: 0.2)
while timerValid == nil, Date() < deadline {
    _ = RunLoop.current.run(mode: .default, before: deadline)
}

print(
    "target=\(targetID) owner=\(targetOwnerName) siblings=\(siblingIDs) "
        + "visible_corners=\(visibleCorners) committed_valid=\(String(describing: timerValid))"
)
fflush(stdout)

precondition(timerValid == true, "The committed panel placement violates sibling occlusion")
