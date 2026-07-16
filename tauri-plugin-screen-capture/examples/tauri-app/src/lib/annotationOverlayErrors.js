export function createAnnotationOverlayErrorPolicy({
  output,
  closeOverlay,
  log = console.error,
}) {
  const describe = (error) => error instanceof Error ? error.message : String(error)

  const reportBoardError = (error) => {
    log(error)
    output.textContent = describe(error)
    output.hidden = false
  }

  const closeFatal = async (error) => {
    log(error)
    await closeOverlay()
  }

  return { reportBoardError, closeFatal }
}
