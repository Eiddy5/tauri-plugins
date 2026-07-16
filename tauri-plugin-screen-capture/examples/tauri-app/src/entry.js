const params = new URLSearchParams(window.location.search)

if (params.get("annotationOverlay") === "1") {
  await import("./annotationOverlayPage.js")
} else {
  await import("./main.js")
}
