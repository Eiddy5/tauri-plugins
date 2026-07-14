#!/usr/bin/env swift

import AppKit
import CoreGraphics
import Darwin

let app = NSApplication.shared
app.setActivationPolicy(.accessory)

func windowRows() -> [[String: Any]] {
    CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
}

let processID = getpid()
guard let target = windowRows().first(where: {
    ($0[kCGWindowOwnerPID as String] as? Int32) != processID
        && ($0[kCGWindowLayer as String] as? Int) == 0
        && (($0[kCGWindowAlpha as String] as? Double) ?? 0) > 0
}) else {
    fatalError("No external layer-0 window is available for the probe")
}
let targetID = target[kCGWindowNumber as String] as! Int

var panels: [NSPanel] = []
for index in 0 ..< 4 {
    let panel = NSPanel(
        contentRect: NSRect(x: 50 + index * 60, y: 50, width: 40, height: 40),
        styleMask: [.borderless, .nonactivatingPanel],
        backing: .buffered,
        defer: false
    )
    panel.title = "TAURI_OVERLAY_ORDER_PROBE_\(index)"
    panel.backgroundColor = .green
    panel.level = .normal
    panels.append(panel)
}
defer {
    for panel in panels {
        panel.orderOut(nil)
    }
}

let panelIDs = Set(panels.map(\.windowNumber))

func relativeOrderIsValid() -> Bool {
    let rows = windowRows()
    guard let targetIndex = rows.firstIndex(where: {
        ($0[kCGWindowNumber as String] as? Int) == targetID
    }), targetIndex >= panelIDs.count else {
        return false
    }
    let immediatelyAbove = rows[(targetIndex - panelIDs.count) ..< targetIndex]
    return Set(immediatelyAbove.compactMap {
        $0[kCGWindowNumber as String] as? Int
    }) == panelIDs && immediatelyAbove.allSatisfy {
        ($0[kCGWindowLayer as String] as? Int) == 0
    }
}

for panel in panels {
    panel.order(.above, relativeTo: targetID)
}

let immediateValid = relativeOrderIsValid()
var timerValid: Bool?
_ = Timer.scheduledTimer(withTimeInterval: 0.0, repeats: false) { _ in
    timerValid = relativeOrderIsValid()
}

let deadline = Date(timeIntervalSinceNow: 0.2)
while timerValid == nil, Date() < deadline {
    _ = RunLoop.current.run(mode: .default, before: deadline)
}

print(
    "immediate_valid=\(immediateValid) "
        + "zero_timer_valid=\(String(describing: timerValid)) "
        + "target=\(targetID) panels=\(panelIDs.sorted())"
)

precondition(!immediateValid, "WindowServer unexpectedly committed ordering synchronously")
precondition(timerValid == true, "The zero-interval timer fired before ordering was committed")
