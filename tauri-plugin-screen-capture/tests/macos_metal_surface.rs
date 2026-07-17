#![cfg(all(target_os = "macos", feature = "macos-screencapturekit"))]

use tauri_plugin_screen_capture::platform::macos::media::{
    MacMetalDevice, MacSurfacePool, SurfacePoolError,
};

#[test]
#[ignore = "requires a logged-in macOS Metal session"]
fn pool_surfaces_are_iosurface_backed_and_metal_compatible() {
    let device = MacMetalDevice::system_default().expect("Metal device");
    let pool = MacSurfacePool::new(&device, 1920, 1080, 3).expect("surface pool");
    let lease = pool.acquire().expect("surface lease");
    assert_eq!(lease.size(), (1920, 1080));
    assert!(lease.pixel_buffer().io_surface().is_some());
    assert!(device.texture_for_bgra(lease.pixel_buffer()).is_ok());
}

#[test]
#[ignore = "requires a logged-in macOS Metal session"]
fn pool_aligns_odd_width_surface_stride_for_metal() {
    let device = MacMetalDevice::system_default().expect("Metal device");
    // 1026 * 4 = 4104, the width that previously triggered Metal's assertion.
    let pool = MacSurfacePool::new(&device, 1026, 64, 1).expect("surface pool");
    let lease = pool.acquire().expect("surface lease");
    assert_eq!(lease.pixel_buffer().bytes_per_row() % 16, 0);
    assert!(device.texture_for_bgra(lease.pixel_buffer()).is_ok());
}

#[test]
#[ignore = "requires a logged-in macOS Metal session"]
fn pool_never_allocates_a_fourth_surface_and_recycles_after_drop() {
    let device = MacMetalDevice::system_default().expect("Metal device");
    let pool = MacSurfacePool::new(&device, 64, 64, 3).expect("surface pool");
    let first = pool.acquire().unwrap();
    let second = pool.acquire().unwrap();
    let third = pool.acquire().unwrap();
    assert!(matches!(
        pool.acquire(),
        Err(SurfacePoolError::Backpressure)
    ));
    drop(first);
    assert!(pool.acquire().is_ok());
    drop((second, third));
}
