use std::sync::Mutex;

use apple_cf::cv::CVPixelBuffer;
use objc2::{msg_send, runtime::AnyObject};
use screencapturekit::metal::{
    IOSurfaceMetalExt, MTLBlendFactor, MTLBlendOperation, MTLLoadAction, MTLPixelFormat,
    MTLPrimitiveType, MTLStoreAction, MetalBuffer, MetalCommandQueue, MetalRenderPassDescriptor,
    MetalRenderPipelineDescriptor, MetalRenderPipelineState, MetalTexture, ResourceOptions,
};

use crate::{
    annotation::{AnnotationSnapshot, Size},
    models::{AnnotationDocument, CaptureErrorCode},
    Error, Result,
};

use super::{
    surface_pool::create_aligned_bgra_surface,
    tessellator::{document_operations, tessellate_operation, StrokeVertex},
    MacCaptureFrame, MacGpuSurface, MacMetalDevice, MacSurfacePool, SurfacePoolError,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct SourceVertex {
    position: [f32; 2],
    uv: [f32; 2],
}

struct CachedAnnotation {
    native_revision: u64,
    document_revision: u64,
    size: (u32, u32),
    _pixel_buffer: CVPixelBuffer,
    texture: MetalTexture,
}

pub struct MacMetalCompositor {
    device: MacMetalDevice,
    queue: MetalCommandQueue,
    pool: MacSurfacePool,
    source_pipeline: MetalRenderPipelineState,
    annotation_pipeline: MetalRenderPipelineState,
    pen_pipeline: MetalRenderPipelineState,
    eraser_pipeline: MetalRenderPipelineState,
    cached_annotation: Mutex<Option<CachedAnnotation>>,
}

impl MacMetalCompositor {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let device = MacMetalDevice::system_default()
            .ok_or_else(|| metal_error("Metal device unavailable"))?;
        let queue = device
            .inner()
            .create_command_queue()
            .ok_or_else(|| metal_error("failed to create Metal command queue"))?;
        let library = device
            .inner()
            .create_library_with_source(include_str!("shaders.metal"))
            .map_err(|message| {
                metal_error(&format!("failed to compile annotation shaders: {message}"))
            })?;
        let source_vertex = library
            .get_function("source_vertex")
            .ok_or_else(|| metal_error("source vertex shader unavailable"))?;
        let source_fragment = library
            .get_function("source_fragment")
            .ok_or_else(|| metal_error("source fragment shader unavailable"))?;
        let stroke_vertex = library
            .get_function("stroke_vertex")
            .ok_or_else(|| metal_error("stroke vertex shader unavailable"))?;
        let stroke_fragment = library
            .get_function("stroke_fragment")
            .ok_or_else(|| metal_error("stroke fragment shader unavailable"))?;

        let source_pipeline = pipeline(
            &device,
            &source_vertex,
            &source_fragment,
            BlendMode::Disabled,
        )?;
        let annotation_pipeline = pipeline(
            &device,
            &source_vertex,
            &source_fragment,
            BlendMode::Premultiplied,
        )?;
        let pen_pipeline = pipeline(
            &device,
            &stroke_vertex,
            &stroke_fragment,
            BlendMode::Premultiplied,
        )?;
        let eraser_pipeline = pipeline(
            &device,
            &stroke_vertex,
            &stroke_fragment,
            BlendMode::DestinationOut,
        )?;
        let pool = MacSurfacePool::new(&device, width, height, 3).map_err(surface_error)?;

        Ok(Self {
            device,
            queue,
            pool,
            source_pipeline,
            annotation_pipeline,
            pen_pipeline,
            eraser_pipeline,
            cached_annotation: Mutex::new(None),
        })
    }

    pub fn compose(
        &self,
        source: &MacCaptureFrame,
        annotations: &AnnotationSnapshot,
    ) -> Result<MacGpuSurface> {
        self.compose_with_document(source, annotations, 0, None)
    }

    pub(crate) fn compose_with_document(
        &self,
        source: &MacCaptureFrame,
        annotations: &AnnotationSnapshot,
        document_revision: u64,
        document: Option<&AnnotationDocument>,
    ) -> Result<MacGpuSurface> {
        let lease = self.pool.acquire().map_err(surface_error)?;
        let output_texture = self
            .device
            .texture_for_bgra(lease.pixel_buffer())
            .map_err(surface_error)?;
        let source_texture = self
            .device
            .texture_for_bgra(source.pixel_buffer())
            .map_err(surface_error)?;
        let (width, height) = lease.size();
        let annotation_texture =
            self.annotation_texture(annotations, document_revision, document, width, height)?;

        let command = self
            .queue
            .command_buffer()
            .ok_or_else(|| metal_error("failed to create Metal command buffer"))?;
        let pass = render_pass(&output_texture, [0.0, 0.0, 0.0, 1.0]);
        let encoder = command
            .render_command_encoder(&pass)
            .ok_or_else(|| metal_error("failed to create Metal render encoder"))?;

        let content = source.geometry().output_content_rect();
        let source_vertices = quad_vertices(
            content.x as f32,
            content.y as f32,
            content.width as f32,
            content.height as f32,
            width as f32,
            height as f32,
        );
        let source_buffer = upload_slice(self.device.inner(), &source_vertices)?;
        encoder.set_render_pipeline_state(&self.source_pipeline);
        encoder.set_vertex_buffer(&source_buffer, 0, 0);
        encoder.set_fragment_texture(&source_texture, 0);
        encoder.draw_primitives(MTLPrimitiveType::Triangle, 0, source_vertices.len());

        let overlay_vertices = quad_vertices(
            0.0,
            0.0,
            width as f32,
            height as f32,
            width as f32,
            height as f32,
        );
        let overlay_buffer = upload_slice(self.device.inner(), &overlay_vertices)?;
        encoder.set_render_pipeline_state(&self.annotation_pipeline);
        encoder.set_vertex_buffer(&overlay_buffer, 0, 0);
        encoder.set_fragment_texture(&annotation_texture, 0);
        encoder.draw_primitives(MTLPrimitiveType::Triangle, 0, overlay_vertices.len());
        encoder.end_encoding();
        command.commit();
        // VideoToolbox is an external IOSurface consumer and does not participate in this
        // command queue's dependency graph. Finish GPU writes before handing it the surface.
        // This synchronizes GPU ownership without mapping or reading pixels on the CPU.
        unsafe {
            let _: () = msg_send![command.as_ptr().cast::<AnyObject>(), waitUntilCompleted];
        }

        Ok(MacGpuSurface::new(
            lease,
            vec![output_texture, source_texture, annotation_texture],
            source.timestamp_ns(),
        ))
    }

    pub fn available_surfaces(&self) -> usize {
        self.pool.available()
    }

    fn annotation_texture(
        &self,
        snapshot: &AnnotationSnapshot,
        document_revision: u64,
        document: Option<&AnnotationDocument>,
        width: u32,
        height: u32,
    ) -> Result<MetalTexture> {
        let mut cached = self
            .cached_annotation
            .lock()
            .map_err(|_| metal_error("annotation texture lock was poisoned"))?;
        if let Some(cached) = cached.as_ref() {
            if cached.native_revision == snapshot.revision()
                && cached.document_revision == document_revision
                && cached.size == (width, height)
            {
                return Ok(cached.texture.clone());
            }
        }

        let surface = create_aligned_bgra_surface(width, height)
            .ok_or_else(|| metal_error("failed to allocate annotation IOSurface"))?;
        let pixel_buffer = CVPixelBuffer::create_with_io_surface(&surface)
            .map_err(|_| metal_error("failed to create annotation pixel buffer"))?;
        let texture = surface
            .create_metal_textures(self.device.inner())
            .map(|textures| textures.plane0)
            .ok_or_else(|| metal_error("failed to create annotation Metal texture"))?;
        let command = self
            .queue
            .command_buffer()
            .ok_or_else(|| metal_error("failed to create annotation command buffer"))?;
        let pass = render_pass(&texture, [0.0, 0.0, 0.0, 0.0]);
        let encoder = command
            .render_command_encoder(&pass)
            .ok_or_else(|| metal_error("failed to create annotation render encoder"))?;

        let document_operations = document
            .map(|document| document_operations(document, Size::new(width as f64, height as f64)))
            .transpose()?
            .unwrap_or_default();
        for operation in snapshot.operations().iter().chain(&document_operations) {
            let mesh = tessellate_operation(operation, Size::new(width as f64, height as f64));
            if mesh.vertices().is_empty() {
                continue;
            }
            let clip_vertices = mesh
                .vertices()
                .iter()
                .map(|vertex| StrokeVertex {
                    position: pixel_to_clip(vertex.position, width as f32, height as f32),
                    color: vertex.color,
                })
                .collect::<Vec<_>>();
            let buffer = upload_slice(self.device.inner(), &clip_vertices)?;
            encoder.set_render_pipeline_state(if mesh.is_eraser() {
                &self.eraser_pipeline
            } else {
                &self.pen_pipeline
            });
            encoder.set_vertex_buffer(&buffer, 0, 0);
            encoder.draw_primitives(MTLPrimitiveType::Triangle, 0, clip_vertices.len());
        }
        encoder.end_encoding();
        command.commit();

        *cached = Some(CachedAnnotation {
            native_revision: snapshot.revision(),
            document_revision,
            size: (width, height),
            _pixel_buffer: pixel_buffer,
            texture: texture.clone(),
        });
        Ok(texture)
    }
}

enum BlendMode {
    Disabled,
    Premultiplied,
    DestinationOut,
}

fn pipeline(
    device: &MacMetalDevice,
    vertex: &screencapturekit::metal::MetalFunction,
    fragment: &screencapturekit::metal::MetalFunction,
    blend: BlendMode,
) -> Result<MetalRenderPipelineState> {
    let descriptor = MetalRenderPipelineDescriptor::new();
    descriptor.set_vertex_function(vertex);
    descriptor.set_fragment_function(fragment);
    descriptor.set_color_attachment_pixel_format(0, MTLPixelFormat::BGRA8Unorm);
    match blend {
        BlendMode::Disabled => {}
        BlendMode::Premultiplied => {
            descriptor.set_blending_enabled(0, true);
            descriptor.set_blend_operations(0, MTLBlendOperation::Add, MTLBlendOperation::Add);
            descriptor.set_blend_factors(
                0,
                MTLBlendFactor::One,
                MTLBlendFactor::OneMinusSourceAlpha,
                MTLBlendFactor::One,
                MTLBlendFactor::OneMinusSourceAlpha,
            );
        }
        BlendMode::DestinationOut => {
            descriptor.set_blending_enabled(0, true);
            descriptor.set_blend_operations(0, MTLBlendOperation::Add, MTLBlendOperation::Add);
            descriptor.set_blend_factors(
                0,
                MTLBlendFactor::Zero,
                MTLBlendFactor::OneMinusSourceAlpha,
                MTLBlendFactor::Zero,
                MTLBlendFactor::OneMinusSourceAlpha,
            );
        }
    }
    device
        .inner()
        .create_render_pipeline_state(&descriptor)
        .ok_or_else(|| metal_error("failed to create Metal render pipeline"))
}

fn render_pass(texture: &MetalTexture, clear: [f64; 4]) -> MetalRenderPassDescriptor {
    let pass = MetalRenderPassDescriptor::new();
    pass.set_color_attachment_texture(0, texture);
    pass.set_color_attachment_load_action(0, MTLLoadAction::Clear);
    pass.set_color_attachment_store_action(0, MTLStoreAction::Store);
    pass.set_color_attachment_clear_color(0, clear[0], clear[1], clear[2], clear[3]);
    pass
}

fn upload_slice<T: Copy>(
    device: &screencapturekit::metal::MetalDevice,
    values: &[T],
) -> Result<MetalBuffer> {
    let size = std::mem::size_of_val(values);
    let buffer = device
        .create_buffer(size, ResourceOptions::STORAGE_MODE_SHARED)
        .ok_or_else(|| metal_error("failed to allocate Metal vertex buffer"))?;
    unsafe {
        std::ptr::copy_nonoverlapping(values.as_ptr().cast::<u8>(), buffer.contents().cast(), size);
    }
    Ok(buffer)
}

fn quad_vertices(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    output_width: f32,
    output_height: f32,
) -> [SourceVertex; 6] {
    let top_left = pixel_to_clip([x, y], output_width, output_height);
    let bottom_right = pixel_to_clip([x + width, y + height], output_width, output_height);
    let left = top_left[0];
    let top = top_left[1];
    let right = bottom_right[0];
    let bottom = bottom_right[1];
    [
        SourceVertex {
            position: [left, top],
            uv: [0.0, 0.0],
        },
        SourceVertex {
            position: [left, bottom],
            uv: [0.0, 1.0],
        },
        SourceVertex {
            position: [right, top],
            uv: [1.0, 0.0],
        },
        SourceVertex {
            position: [right, top],
            uv: [1.0, 0.0],
        },
        SourceVertex {
            position: [left, bottom],
            uv: [0.0, 1.0],
        },
        SourceVertex {
            position: [right, bottom],
            uv: [1.0, 1.0],
        },
    ]
}

fn pixel_to_clip(point: [f32; 2], width: f32, height: f32) -> [f32; 2] {
    [point[0] / width * 2.0 - 1.0, 1.0 - point[1] / height * 2.0]
}

fn surface_error(error: SurfacePoolError) -> Error {
    Error::new(
        CaptureErrorCode::CaptureRuntimeFailed,
        error.to_string(),
        true,
    )
}

fn metal_error(message: &str) -> Error {
    Error::new(CaptureErrorCode::CaptureRuntimeFailed, message, true)
}
