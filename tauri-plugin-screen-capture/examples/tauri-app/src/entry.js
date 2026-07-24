const params = new URLSearchParams(window.location.search)

if (params.get("nativeAnnotationToolbar") === "1") {
  await import("./nativeAnnotationToolbarPage.js")
} else {
  await import("./main.js")
}
