use std::{
    sync::mpsc::{self, Sender},
    time::Duration,
};

use windows::Win32::{
    Foundation::RECT,
    Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO},
};
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl},
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
};

use crate::{
    error::{Error, ErrorCode},
    Result,
};

const FRAME_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct MonitorFrame {
    rect: RECT,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

struct OneShotCapture {
    sender: Sender<std::result::Result<MonitorFrame, String>>,
    rect: RECT,
    sent: bool,
}

impl GraphicsCaptureApiHandler for OneShotCapture {
    type Flags = (Sender<std::result::Result<MonitorFrame, String>>, RECT);
    type Error = String;

    fn new(context: Context<Self::Flags>) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            sender: context.flags.0,
            rect: context.flags.1,
            sent: false,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> std::result::Result<(), Self::Error> {
        if self.sent {
            return Ok(());
        }
        self.sent = true;

        let result = (|| {
            let width = frame.width();
            let height = frame.height();
            let frame_buffer = frame.buffer().map_err(|error| error.to_string())?;
            let mut packed = Vec::new();
            let pixels = frame_buffer.as_nopadding_buffer(&mut packed).to_vec();
            Ok(MonitorFrame {
                rect: self.rect,
                width,
                height,
                pixels,
            })
        })();

        let _ = self.sender.send(result);
        capture_control.stop();
        Ok(())
    }
}

pub(super) fn capture_virtual_desktop(desktop: RECT, width: u32, height: u32) -> Result<Vec<u8>> {
    match GraphicsCaptureApi::is_supported() {
        Ok(true) => {}
        Ok(false) => {
            return Err(capture_error(
                "Windows Graphics Capture 在当前系统上不可用（要求 Windows 10 1903 或更高版本）",
            ));
        }
        Err(error) => {
            return Err(capture_error(format!(
                "检查 Windows Graphics Capture 支持状态失败：{error}"
            )));
        }
    }

    let monitors = Monitor::enumerate()
        .map_err(|error| capture_error(format!("枚举 Windows 显示器失败：{error}")))?;
    if monitors.is_empty() {
        return Err(capture_error("Windows Graphics Capture 未找到活动显示器"));
    }

    let (sender, receiver) = mpsc::channel();
    let mut controls: Vec<CaptureControl<OneShotCapture, String>> =
        Vec::with_capacity(monitors.len());
    for monitor in monitors {
        let rect = monitor_rect(monitor)?;
        let settings = Settings::new(
            monitor,
            CursorCaptureSettings::WithoutCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Exclude,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            (sender.clone(), rect),
        );
        let control = match OneShotCapture::start_free_threaded(settings) {
            Ok(control) => control,
            Err(error) => {
                for control in controls {
                    let _ = control.stop();
                }
                return Err(capture_error(format!(
                    "启动 Windows Graphics Capture 会话失败：{error}"
                )));
            }
        };
        controls.push(control);
    }
    drop(sender);

    let mut frames = Vec::with_capacity(controls.len());
    for _ in 0..controls.len() {
        let frame = match receiver.recv_timeout(FRAME_TIMEOUT) {
            Ok(Ok(frame)) => frame,
            Ok(Err(error)) => {
                for control in controls {
                    let _ = control.stop();
                }
                return Err(capture_error(error));
            }
            Err(_) => {
                for control in controls {
                    let _ = control.stop();
                }
                return Err(capture_error("等待 Windows Graphics Capture 首帧超时"));
            }
        };
        frames.push(frame);
    }
    let mut close_error = None;
    for control in controls {
        if let Err(error) = control.wait() {
            close_error.get_or_insert_with(|| {
                capture_error(format!("关闭 Windows Graphics Capture 会话失败：{error}"))
            });
        }
    }
    if let Some(error) = close_error {
        return Err(error);
    }

    compose_virtual_desktop(desktop, width, height, frames)
}

fn monitor_rect(monitor: Monitor) -> Result<RECT> {
    let monitor = HMONITOR(monitor.as_raw_hmonitor());
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return Err(capture_error("Windows Graphics Capture 无法读取显示器坐标"));
    }
    Ok(info.rcMonitor)
}

fn compose_virtual_desktop(
    desktop: RECT,
    width: u32,
    height: u32,
    frames: Vec<MonitorFrame>,
) -> Result<Vec<u8>> {
    let stride = width as usize * 4;
    let len = stride
        .checked_mul(height as usize)
        .ok_or_else(|| capture_error("Windows 虚拟桌面像素缓冲区过大"))?;
    let mut output = vec![0; len];

    for frame in frames {
        let expected = frame.width as usize * frame.height as usize * 4;
        if frame.pixels.len() != expected {
            return Err(capture_error(
                "Windows Graphics Capture 返回了无效的像素缓冲区",
            ));
        }
        let target_x = frame.rect.left.saturating_sub(desktop.left).max(0) as usize;
        let target_y = frame.rect.top.saturating_sub(desktop.top).max(0) as usize;
        let copy_width = frame.width.min(width.saturating_sub(target_x as u32)) as usize;
        let copy_height = frame.height.min(height.saturating_sub(target_y as u32)) as usize;
        let row_bytes = copy_width * 4;
        let source_stride = frame.width as usize * 4;

        for row in 0..copy_height {
            let source_start = row * source_stride;
            let target_start = (target_y + row) * stride + target_x * 4;
            output[target_start..target_start + row_bytes]
                .copy_from_slice(&frame.pixels[source_start..source_start + row_bytes]);
        }
    }
    Ok(output)
}

fn capture_error(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::CaptureFailed, message, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composes_monitors_with_negative_virtual_desktop_coordinates() {
        let desktop = RECT {
            left: -2,
            top: 0,
            right: 2,
            bottom: 1,
        };
        let left = MonitorFrame {
            rect: RECT {
                left: -2,
                top: 0,
                right: 0,
                bottom: 1,
            },
            width: 2,
            height: 1,
            pixels: vec![1; 8],
        };
        let right = MonitorFrame {
            rect: RECT {
                left: 0,
                top: 0,
                right: 2,
                bottom: 1,
            },
            width: 2,
            height: 1,
            pixels: vec![2; 8],
        };

        let composed = compose_virtual_desktop(desktop, 4, 1, vec![right, left]).unwrap();

        assert_eq!(&composed[..8], &[1; 8]);
        assert_eq!(&composed[8..], &[2; 8]);
    }
}
