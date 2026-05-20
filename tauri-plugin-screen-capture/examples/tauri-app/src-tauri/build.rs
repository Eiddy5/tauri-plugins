fn main() {
  add_swift_runtime_rpath();
  tauri_build::build()
}

fn add_swift_runtime_rpath() {
  if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("macos") {
    return;
  }

  println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
