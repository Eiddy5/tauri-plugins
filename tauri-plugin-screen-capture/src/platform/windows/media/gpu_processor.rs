use std::{mem::ManuallyDrop, ptr};

use windows::{
    core::Interface,
    Win32::{
        Foundation::RECT,
        Graphics::{
            Direct3D11::{
                ID3D11Multithread, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice,
                ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
                ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET, D3D11_TEX2D_VPIV,
                D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
                D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
                D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
                D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
                D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
                D3D11_VPOV_DIMENSION_TEXTURE2D,
            },
            Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_RATIONAL},
        },
    },
};
use windows_capture::frame::Frame;

use crate::{platform::windows::media::WindowsGpuSurface, Error, Result};

const GPU_SURFACE_POOL_SIZE: usize = 4;

pub(crate) struct D3D11Nv12Processor {
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    surfaces: Vec<WindowsGpuSurface>,
    next_surface: usize,
}

impl D3D11Nv12Processor {
    pub(crate) fn new(frame: &Frame<'_>, target_width: u32, target_height: u32) -> Result<Self> {
        let multithread: ID3D11Multithread = frame.device().cast().map_err(gpu_error)?;
        let _ = unsafe { multithread.SetMultithreadProtected(true) };

        let source_width = frame.width();
        let source_height = frame.height();
        let video_device: ID3D11VideoDevice = frame.device().cast().map_err(gpu_error)?;
        let video_context: ID3D11VideoContext = frame.device_context().cast().map_err(gpu_error)?;
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            InputWidth: source_width,
            InputHeight: source_height,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            OutputWidth: target_width,
            OutputHeight: target_height,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let enumerator =
            unsafe { video_device.CreateVideoProcessorEnumerator(&content) }.map_err(gpu_error)?;
        let format_support =
            unsafe { enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_NV12) }.map_err(gpu_error)?;
        if format_support & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0 as u32 == 0 {
            return Err(gpu_error(
                "D3D11 video processor does not support NV12 output",
            ));
        }
        let processor =
            unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }.map_err(gpu_error)?;

        let source = frame.desc();
        let output_desc = D3D11_TEXTURE2D_DESC {
            Width: target_width,
            Height: target_height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: source.SampleDesc,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let output_view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut surfaces = Vec::with_capacity(GPU_SURFACE_POOL_SIZE);
        for _ in 0..GPU_SURFACE_POOL_SIZE {
            let mut texture = None;
            unsafe {
                frame
                    .device()
                    .CreateTexture2D(&output_desc, None, Some(&mut texture))
            }
            .map_err(gpu_error)?;
            let texture = texture.ok_or_else(|| gpu_error("no NV12 output texture"))?;
            let mut output_view = None;
            unsafe {
                video_device.CreateVideoProcessorOutputView(
                    &texture,
                    &enumerator,
                    &output_view_desc,
                    Some(&mut output_view),
                )
            }
            .map_err(gpu_error)?;
            let output_view =
                output_view.ok_or_else(|| gpu_error("no NV12 video processor output view"))?;
            surfaces.push(WindowsGpuSurface::new(
                frame.device().clone(),
                texture,
                output_view,
                target_width,
                target_height,
            ));
        }

        let source_rect = RECT {
            left: 0,
            top: 0,
            right: source_width as i32,
            bottom: source_height as i32,
        };
        let target_rect = RECT {
            left: 0,
            top: 0,
            right: target_width as i32,
            bottom: target_height as i32,
        };
        unsafe {
            video_context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&target_rect));
            video_context.VideoProcessorSetStreamSourceRect(
                &processor,
                0,
                true,
                Some(&source_rect),
            );
            video_context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&target_rect));
        }

        Ok(Self {
            source_width,
            source_height,
            target_width,
            target_height,
            video_device,
            video_context,
            enumerator,
            processor,
            surfaces,
            next_surface: 0,
        })
    }

    pub(crate) fn matches(
        &self,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
    ) -> bool {
        self.source_width == source_width
            && self.source_height == source_height
            && self.target_width == target_width
            && self.target_height == target_height
    }

    pub(crate) fn process(
        &mut self,
        frame: &Frame<'_>,
        timestamp_ns: u64,
    ) -> Result<Option<WindowsGpuSurface>> {
        let surface_index = (0..self.surfaces.len())
            .map(|offset| (self.next_surface + offset) % self.surfaces.len())
            .find(|index| self.surfaces[*index].is_available());
        let Some(surface_index) = surface_index else {
            return Ok(None);
        };

        let input_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let mut input_view = None;
        unsafe {
            self.video_device.CreateVideoProcessorInputView(
                frame.as_raw_texture(),
                &self.enumerator,
                &input_desc,
                Some(&mut input_view),
            )
        }
        .map_err(gpu_error)?;
        let input_view = input_view.ok_or_else(|| gpu_error("no GPU surface input view"))?;
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(Some(input_view)),
            ppPastSurfaces: ptr::null_mut(),
            ppFutureSurfaces: ptr::null_mut(),
            ppPastSurfacesRight: ptr::null_mut(),
            ppFutureSurfacesRight: ptr::null_mut(),
            pInputSurfaceRight: ManuallyDrop::new(None),
            ..D3D11_VIDEO_PROCESSOR_STREAM::default()
        };
        let result = unsafe {
            self.video_context.VideoProcessorBlt(
                &self.processor,
                self.surfaces[surface_index].output_view(),
                0,
                std::slice::from_ref(&stream),
            )
        };
        unsafe {
            ManuallyDrop::drop(&mut stream.pInputSurface);
            ManuallyDrop::drop(&mut stream.pInputSurfaceRight);
        }
        result.map_err(gpu_error)?;
        self.next_surface = (surface_index + 1) % self.surfaces.len();
        Ok(Some(
            self.surfaces[surface_index].with_timestamp(timestamp_ns),
        ))
    }
}

pub(crate) struct D3D11BgraScaler {
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    output_texture: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
}

impl D3D11BgraScaler {
    pub(crate) fn new(frame: &Frame<'_>, target_width: u32, target_height: u32) -> Result<Self> {
        let source_width = frame.width();
        let source_height = frame.height();
        let video_device: ID3D11VideoDevice = frame.device().cast().map_err(gpu_error)?;
        let video_context: ID3D11VideoContext = frame.device_context().cast().map_err(gpu_error)?;
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            InputWidth: source_width,
            InputHeight: source_height,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            OutputWidth: target_width,
            OutputHeight: target_height,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let enumerator =
            unsafe { video_device.CreateVideoProcessorEnumerator(&content) }.map_err(gpu_error)?;
        let processor =
            unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }.map_err(gpu_error)?;

        let source = frame.desc();
        let output_desc = D3D11_TEXTURE2D_DESC {
            Width: target_width,
            Height: target_height,
            MipLevels: 1,
            ArraySize: 1,
            Format: source.Format,
            SampleDesc: source.SampleDesc,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut output_texture = None;
        unsafe {
            frame
                .device()
                .CreateTexture2D(&output_desc, None, Some(&mut output_texture))
        }
        .map_err(gpu_error)?;
        let output_texture = output_texture.ok_or_else(|| gpu_error("no scaler output texture"))?;
        let output_view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut output_view = None;
        unsafe {
            video_device.CreateVideoProcessorOutputView(
                &output_texture,
                &enumerator,
                &output_view_desc,
                Some(&mut output_view),
            )
        }
        .map_err(gpu_error)?;
        let output_view = output_view.ok_or_else(|| gpu_error("no scaler output view"))?;

        let source_rect = RECT {
            left: 0,
            top: 0,
            right: source_width as i32,
            bottom: source_height as i32,
        };
        let target_rect = RECT {
            left: 0,
            top: 0,
            right: target_width as i32,
            bottom: target_height as i32,
        };
        unsafe {
            video_context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&target_rect));
            video_context.VideoProcessorSetStreamSourceRect(
                &processor,
                0,
                true,
                Some(&source_rect),
            );
            video_context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&target_rect));
        }

        Ok(Self {
            source_width,
            source_height,
            target_width,
            target_height,
            video_device,
            video_context,
            enumerator,
            processor,
            output_texture,
            output_view,
        })
    }

    pub(crate) fn matches(
        &self,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
    ) -> bool {
        self.source_width == source_width
            && self.source_height == source_height
            && self.target_width == target_width
            && self.target_height == target_height
    }

    pub(crate) fn scale(&self, frame: &Frame<'_>) -> Result<ID3D11Texture2D> {
        let input_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let mut input_view = None;
        unsafe {
            self.video_device.CreateVideoProcessorInputView(
                frame.as_raw_texture(),
                &self.enumerator,
                &input_desc,
                Some(&mut input_view),
            )
        }
        .map_err(gpu_error)?;
        let input_view = input_view.ok_or_else(|| gpu_error("no scaler input view"))?;
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(Some(input_view)),
            ppPastSurfaces: ptr::null_mut(),
            ppFutureSurfaces: ptr::null_mut(),
            ppPastSurfacesRight: ptr::null_mut(),
            ppFutureSurfacesRight: ptr::null_mut(),
            pInputSurfaceRight: ManuallyDrop::new(None),
            ..D3D11_VIDEO_PROCESSOR_STREAM::default()
        };
        let result = unsafe {
            self.video_context.VideoProcessorBlt(
                &self.processor,
                &self.output_view,
                0,
                std::slice::from_ref(&stream),
            )
        };
        unsafe {
            ManuallyDrop::drop(&mut stream.pInputSurface);
            ManuallyDrop::drop(&mut stream.pInputSurfaceRight);
        }
        result.map_err(gpu_error)?;
        Ok(self.output_texture.clone())
    }
}

fn gpu_error(error: impl std::fmt::Display) -> Error {
    Error::new(
        crate::CaptureErrorCode::CaptureRuntimeFailed,
        format!("D3D11 video processor scaling failed: {error}"),
        true,
    )
}
