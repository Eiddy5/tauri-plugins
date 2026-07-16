export async function applyAnnotationTargetGeometry(window, target, dpi) {
  if (!target) {
    await window.hide()
    return
  }

  const logical = target.coordinateSpace === "logical"
  const Position = logical ? dpi.LogicalPosition : dpi.PhysicalPosition
  const Size = logical ? dpi.LogicalSize : dpi.PhysicalSize
  await window.setPosition(new Position(target.x, target.y))
  await window.setSize(new Size(target.width, target.height))
  await window.show()
}

export function createAnnotationTargetSynchronizer(window, dpi) {
  let lastGeometry = ""
  let focused = false

  return async (target) => {
    if (!target) {
      await applyAnnotationTargetGeometry(window, null, dpi)
      lastGeometry = ""
      focused = false
      return
    }

    const geometry = JSON.stringify(target)
    if (geometry !== lastGeometry) {
      await applyAnnotationTargetGeometry(window, target, dpi)
      lastGeometry = geometry
    }
    if (!focused) {
      await window.setFocus()
      focused = true
    }
  }
}
