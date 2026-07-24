export async function applyNativeAnnotationToolbarGeometry(window, target, dpi, toolbar) {
  if (!target) {
    await window.hide()
    return
  }

  const width = Math.min(toolbar.width, target.width)
  const height = Math.min(toolbar.height, target.height)
  const x = target.x + (target.width - width) / 2
  const y = target.y + Math.min(toolbar.topInset, Math.max(0, target.height - height))
  const logical = target.coordinateSpace === "logical"
  const Position = logical ? dpi.LogicalPosition : dpi.PhysicalPosition
  const Size = logical ? dpi.LogicalSize : dpi.PhysicalSize
  await window.setPosition(new Position(x, y))
  await window.setSize(new Size(width, height))
  await window.show()
}

export function createNativeAnnotationToolbarSynchronizer(window, dpi, toolbar) {
  let lastGeometry = ""
  let focused = false

  return async (target) => {
    if (!target) {
      await applyNativeAnnotationToolbarGeometry(window, null, dpi, toolbar)
      lastGeometry = ""
      focused = false
      return
    }

    const geometry = JSON.stringify(target)
    if (geometry !== lastGeometry) {
      await applyNativeAnnotationToolbarGeometry(window, target, dpi, toolbar)
      lastGeometry = geometry
    }
    if (!focused) {
      await window.setFocus()
      focused = true
    }
  }
}
