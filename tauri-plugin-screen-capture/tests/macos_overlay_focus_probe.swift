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

func orderIsValid(_ ids: [CGWindowID], target: CGWindowID, panels: [CGWindowID]) -> Bool {
    guard let targetIndex = ids.firstIndex(of: target), targetIndex >= panels.count else {
        return false
    }
    let group = ids[(targetIndex - panels.count)..<targetIndex]
    return panels.allSatisfy { panel in group.filter { $0 == panel }.count == 1 }
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
    panel.order(.above, relativeTo: target.windowNumber)
    return panel
}

advanceWindowServer()
let targetID = CGWindowID(target.windowNumber)
let panelIDs = panels.map { CGWindowID($0.windowNumber) }
precondition(
    orderIsValid(visibleWindowIDs(), target: targetID, panels: panelIDs),
    "initial panel order was not established"
)

target.orderFrontRegardless()
advanceWindowServer()
precondition(
    !orderIsValid(visibleWindowIDs(), target: targetID, panels: panelIDs),
    "bringing the target forward did not reproduce panel order loss"
)

for panel in panels {
    panel.order(.above, relativeTo: target.windowNumber)
}
advanceWindowServer()
precondition(
    orderIsValid(visibleWindowIDs(), target: targetID, panels: panelIDs),
    "relative panel reorder did not restore the invariant"
)

for panel in panels {
    panel.close()
}
target.close()
print("PASS: focus reorder reproduced and repaired")
