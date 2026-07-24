#[cfg(target_os = "macos")]
mod macos {
    use std::{
        ffi::c_void,
        ffi::CString,
        ptr,
        sync::mpsc::{self, Receiver, Sender},
        time::Duration,
    };

    use apple_cf::{
        cm::{CMSampleBuffer, CMTime},
        cv::CVPixelBuffer,
    };

    use crate::{
        error::Error,
        models::{CaptureErrorCode, PixelFormat},
        pipeline::frame::VideoFrame,
        webrtc::track::EncodedVideoSample,
        Result,
    };

    #[cfg(feature = "macos-screencapturekit")]
    use crate::platform::macos::media::MacGpuSurface;

    const K_CM_VIDEO_CODEC_TYPE_H264: u32 = u32::from_be_bytes(*b"avc1");
    const K_CV_PIXEL_FORMAT_TYPE_32_BGRA: u32 = u32::from_be_bytes(*b"BGRA");

    pub(crate) struct H264Encoder {
        session: VTCompressionSessionRef,
        context: *mut EncoderContext,
        receiver: Receiver<EncodedVideoSample>,
        frame_duration: Duration,
        target_bitrate_bps: u32,
        force_next_keyframe: bool,
    }

    struct EncoderContext {
        sender: Sender<EncodedVideoSample>,
        frame_duration: Duration,
    }

    unsafe impl Send for H264Encoder {}

    impl H264Encoder {
        pub fn new(width: u32, height: u32, fps: u32) -> Result<Self> {
            let (sender, receiver) = mpsc::channel();
            let frame_duration = Duration::from_secs_f64(1.0 / f64::from(fps.max(1)));
            let target_bitrate_bps = recommended_bitrate(width, height, fps) as u32;
            let context = Box::into_raw(Box::new(EncoderContext {
                sender,
                frame_duration,
            }));
            let mut session = ptr::null_mut();
            let status = unsafe {
                VTCompressionSessionCreate(
                    ptr::null(),
                    width as i32,
                    height as i32,
                    K_CM_VIDEO_CODEC_TYPE_H264,
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    Some(compression_output_callback),
                    context.cast(),
                    &mut session,
                )
            };
            if status != 0 || session.is_null() {
                unsafe {
                    drop(Box::from_raw(context));
                }
                return Err(video_toolbox_error(
                    "failed to create VideoToolbox compression session",
                    status,
                ));
            }

            if let Err(error) = configure_session(session, width, height, fps) {
                unsafe {
                    VTCompressionSessionInvalidate(session);
                    CFRelease(session.cast());
                    drop(Box::from_raw(context));
                }
                return Err(error);
            }

            let status = unsafe { VTCompressionSessionPrepareToEncodeFrames(session) };
            if status != 0 {
                unsafe {
                    VTCompressionSessionInvalidate(session);
                    CFRelease(session.cast());
                    drop(Box::from_raw(context));
                }
                return Err(video_toolbox_error(
                    "failed to prepare VideoToolbox compression session",
                    status,
                ));
            }

            Ok(Self {
                session,
                context,
                receiver,
                frame_duration,
                target_bitrate_bps,
                force_next_keyframe: false,
            })
        }

        pub(crate) fn target_bitrate_bps(&self) -> u32 {
            self.target_bitrate_bps
        }

        pub fn encode_frame(&mut self, frame: &VideoFrame) -> Result<Option<EncodedVideoSample>> {
            if frame.pixel_format != PixelFormat::Bgra {
                return Err(Error::new(
                    CaptureErrorCode::WebRtcTrackFailed,
                    "VideoToolbox H264 encoder currently expects BGRA frames",
                    true,
                ));
            }

            let pixel_buffer = CVPixelBuffer::create(
                frame.width as usize,
                frame.height as usize,
                K_CV_PIXEL_FORMAT_TYPE_32_BGRA,
            )
            .map_err(|status| {
                video_toolbox_error("failed to create encoder pixel buffer", status)
            })?;
            {
                let mut guard = pixel_buffer.lock_read_write().map_err(|status| {
                    video_toolbox_error("failed to lock encoder pixel buffer", status)
                })?;
                let bytes_per_row = guard.bytes_per_row();
                let tight_bytes_per_row = frame.width as usize * 4;
                let destination = guard.as_slice_mut().ok_or_else(|| {
                    video_toolbox_error("encoder pixel buffer was not writable", -1)
                })?;
                for (source_row, destination_row) in frame
                    .data
                    .chunks_exact(tight_bytes_per_row)
                    .zip(destination.chunks_mut(bytes_per_row))
                {
                    destination_row[..tight_bytes_per_row].copy_from_slice(source_row);
                }
            }
            let timestamp = Box::into_raw(Box::new(SubmittedFrame::cpu(frame.timestamp_ns)));
            let pts = CMTime::new(
                i64::try_from(frame.timestamp_ns / 1_000).unwrap_or(i64::MAX),
                1_000_000,
            );
            let duration = CMTime::new(
                i64::try_from(self.frame_duration.as_micros()).unwrap_or(i64::MAX),
                1_000_000,
            );
            let force_keyframe = std::mem::take(&mut self.force_next_keyframe);
            let status = match encode_video_frame(
                self.session,
                pixel_buffer.as_ptr(),
                pts,
                duration,
                timestamp.cast(),
                force_keyframe,
            ) {
                Ok(status) => status,
                Err(error) => {
                    unsafe { drop(Box::from_raw(timestamp)) };
                    return Err(error);
                }
            };
            if status != 0 {
                unsafe {
                    drop(Box::from_raw(timestamp));
                }
                return Err(video_toolbox_error(
                    "failed to encode frame with VideoToolbox",
                    status,
                ));
            }

            Ok(self.receiver.try_iter().last())
        }

        #[cfg(feature = "macos-screencapturekit")]
        pub(crate) fn encode_gpu_surface(&mut self, surface: MacGpuSurface) -> Result<()> {
            let pts = CMTime::new(
                i64::try_from(surface.timestamp_ns() / 1_000).unwrap_or(i64::MAX),
                1_000_000,
            );
            let duration = CMTime::new(
                i64::try_from(self.frame_duration.as_micros()).unwrap_or(i64::MAX),
                1_000_000,
            );
            let submitted = Box::into_raw(Box::new(SubmittedFrame::gpu(surface)));
            let force_keyframe = std::mem::take(&mut self.force_next_keyframe);
            let status = match encode_video_frame(
                self.session,
                unsafe { (*submitted).pixel_buffer_ptr() },
                pts,
                duration,
                submitted.cast(),
                force_keyframe,
            ) {
                Ok(status) => status,
                Err(error) => {
                    unsafe { drop(Box::from_raw(submitted)) };
                    return Err(error);
                }
            };
            if status != 0 {
                unsafe { drop(Box::from_raw(submitted)) };
                return Err(video_toolbox_error(
                    "failed to encode GPU surface with VideoToolbox",
                    status,
                ));
            }
            Ok(())
        }

        #[cfg(feature = "macos-screencapturekit")]
        pub(crate) fn take_encoded_samples(&self) -> Vec<EncodedVideoSample> {
            self.receiver.try_iter().collect()
        }

        #[cfg(feature = "macos-screencapturekit")]
        pub(crate) fn complete_frames(&self) {
            unsafe {
                VTCompressionSessionCompleteFrames(self.session, CMTime::INVALID);
            }
        }

        pub(crate) fn force_keyframe(&mut self) -> Result<()> {
            self.force_next_keyframe = true;
            Ok(())
        }
    }

    struct SubmittedFrame {
        timestamp_ns: u64,
        #[cfg(feature = "macos-screencapturekit")]
        surface: Option<MacGpuSurface>,
    }

    impl SubmittedFrame {
        fn cpu(timestamp_ns: u64) -> Self {
            Self {
                timestamp_ns,
                #[cfg(feature = "macos-screencapturekit")]
                surface: None,
            }
        }

        #[cfg(feature = "macos-screencapturekit")]
        fn gpu(surface: MacGpuSurface) -> Self {
            Self {
                timestamp_ns: surface.timestamp_ns(),
                surface: Some(surface),
            }
        }

        #[cfg(feature = "macos-screencapturekit")]
        fn pixel_buffer_ptr(&self) -> *mut c_void {
            self.surface
                .as_ref()
                .expect("GPU submission owns its surface")
                .pixel_buffer()
                .as_ptr()
        }
    }

    impl Drop for H264Encoder {
        fn drop(&mut self) {
            unsafe {
                VTCompressionSessionCompleteFrames(self.session, CMTime::INVALID);
                VTCompressionSessionInvalidate(self.session);
                CFRelease(self.session.cast());
                drop(Box::from_raw(self.context));
            }
        }
    }

    extern "C" fn compression_output_callback(
        output_callback_ref_con: *mut c_void,
        source_frame_ref_con: *mut c_void,
        status: i32,
        _info_flags: u32,
        sample_buffer: *mut c_void,
    ) {
        let timestamp_ns = if source_frame_ref_con.is_null() {
            0
        } else {
            unsafe { Box::from_raw(source_frame_ref_con.cast::<SubmittedFrame>()).timestamp_ns }
        };
        if status != 0 || output_callback_ref_con.is_null() || sample_buffer.is_null() {
            return;
        }
        let context = unsafe { &*(output_callback_ref_con.cast::<EncoderContext>()) };
        let Some(sample_buffer) = (unsafe { CMSampleBuffer::from_raw_retained(sample_buffer) })
        else {
            return;
        };
        let Some(block_buffer) = sample_buffer.data_buffer() else {
            return;
        };
        let Some(avcc) = block_buffer.copy_data_bytes(0, block_buffer.data_length()) else {
            return;
        };

        let mut annex_b = Vec::with_capacity(avcc.len() + 64);
        if let Some(format_description) = sample_buffer.format_description() {
            append_h264_parameter_sets(format_description.as_ptr(), &mut annex_b);
        }
        append_avcc_as_annex_b(&avcc, &mut annex_b);
        if annex_b.is_empty() {
            return;
        }

        let _ = context.sender.send(EncodedVideoSample {
            data: annex_b,
            duration: context.frame_duration,
            timestamp_ns,
        });
    }

    fn append_h264_parameter_sets(format_description: *mut c_void, output: &mut Vec<u8>) {
        for index in 0..2 {
            let mut parameter_set = ptr::null();
            let mut parameter_set_size = 0;
            let mut parameter_set_count = 0;
            let mut nal_unit_header_length = 0;
            let status = unsafe {
                CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    format_description,
                    index,
                    &mut parameter_set,
                    &mut parameter_set_size,
                    &mut parameter_set_count,
                    &mut nal_unit_header_length,
                )
            };
            if status == 0 && !parameter_set.is_null() && parameter_set_size > 0 {
                output.extend_from_slice(&[0, 0, 0, 1]);
                output.extend_from_slice(unsafe {
                    std::slice::from_raw_parts(parameter_set, parameter_set_size)
                });
            }
        }
    }

    fn append_avcc_as_annex_b(input: &[u8], output: &mut Vec<u8>) {
        let mut offset = 0;
        while offset + 4 <= input.len() {
            let len = u32::from_be_bytes(input[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            if len == 0 || offset + len > input.len() {
                break;
            }
            output.extend_from_slice(&[0, 0, 0, 1]);
            output.extend_from_slice(&input[offset..offset + len]);
            offset += len;
        }
    }

    fn video_toolbox_error(message: &'static str, status: i32) -> Error {
        Error::new(
            CaptureErrorCode::WebRtcTrackFailed,
            format!("{message}: OSStatus {status}"),
            true,
        )
    }

    fn configure_session(
        session: VTCompressionSessionRef,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<()> {
        set_bool_property(session, "RealTime", true)?;
        set_bool_property(session, "AllowFrameReordering", false)?;
        set_i32_property(
            session,
            "ExpectedFrameRate",
            i32::try_from(fps.max(1)).unwrap_or(60),
        )?;
        set_i32_property(
            session,
            "AverageBitRate",
            recommended_bitrate(width, height, fps),
        )?;
        set_i32_property(
            session,
            "MaxKeyFrameInterval",
            i32::try_from(fps.max(1).saturating_mul(5)).unwrap_or(300),
        )?;
        // Screen content benefits substantially from CABAC and the additional
        // prediction modes available in High Profile. Baseline was originally
        // selected for compatibility, but it produces visibly softer text at
        // the same real-time bitrate.
        set_string_property(session, "ProfileLevel", "H264_High_AutoLevel")?;
        Ok(())
    }

    fn recommended_bitrate(width: u32, height: u32, fps: u32) -> i32 {
        let pixels = u64::from(width).saturating_mul(u64::from(height));
        let target_bitrate_bps = pixels
            .saturating_mul(u64::from(fps.max(1)))
            .saturating_mul(16)
            .div_ceil(100);
        i32::try_from(target_bitrate_bps.clamp(8_000_000, 48_000_000)).unwrap_or(32_000_000)
    }

    #[cfg(test)]
    mod tests {
        use super::{recommended_bitrate, H264Encoder};
        use crate::{models::PixelFormat, pipeline::frame::VideoFrame};

        #[test]
        fn bitrate_matches_the_screen_share_transport_budget() {
            assert_eq!(recommended_bitrate(1920, 1080, 60), 19_906_560);
            assert_eq!(recommended_bitrate(2560, 1440, 60), 35_389_440);
            assert_eq!(recommended_bitrate(3840, 2160, 30), 39_813_120);
        }

        #[test]
        fn force_keyframe_makes_the_next_video_toolbox_sample_an_idr() {
            let mut encoder = H264Encoder::new(64, 64, 30).expect("create VideoToolbox encoder");
            let frame = |timestamp_ns| VideoFrame {
                width: 64,
                height: 64,
                pixel_format: PixelFormat::Bgra,
                timestamp_ns,
                data: vec![0x80; 64 * 64 * 4].into(),
            };

            let _ = encoder
                .encode_frame(&frame(0))
                .expect("encode initial frame");
            encoder.force_keyframe().expect("request keyframe");
            let _ = encoder
                .encode_frame(&frame(33_333_333))
                .expect("encode forced frame");
            let status = unsafe {
                super::VTCompressionSessionCompleteFrames(
                    encoder.session,
                    apple_cf::cm::CMTime::INVALID,
                )
            };
            assert_eq!(status, 0);
            let sample = encoder
                .receiver
                .try_iter()
                .last()
                .expect("receive forced frame");

            assert!(sample.is_h264_idr());
        }

        #[cfg(feature = "macos-screencapturekit")]
        #[test]
        #[ignore = "requires a logged-in macOS Metal session"]
        fn videotoolbox_encodes_a_metal_composited_iosurface() {
            use crate::{
                annotation::AnnotationSession,
                platform::macos::media::{
                    CaptureFrameGeometry, MacCaptureFrame, MacMetalCompositor,
                },
            };

            let width = 64_u32;
            let height = 64_u32;
            let bytes_per_row = width as usize * 4;
            let io_surface = apple_cf::iosurface::IOSurface::create_with_properties(
                width as usize,
                height as usize,
                u32::from_be_bytes(*b"BGRA"),
                4,
                bytes_per_row,
                bytes_per_row * height as usize,
                None,
            )
            .expect("source IOSurface");
            let pixel_buffer = apple_cf::cv::CVPixelBuffer::create_with_io_surface(&io_surface)
                .expect("source pixel buffer");
            let geometry =
                CaptureFrameGeometry::from_attachments(width, height, None, None, None).unwrap();
            let source = MacCaptureFrame::new(pixel_buffer, 0, geometry, 1);
            let compositor = MacMetalCompositor::new(width, height).expect("Metal compositor");
            let surface = compositor
                .compose(&source, &AnnotationSession::default().snapshot())
                .expect("compose IOSurface");

            let mut encoder = H264Encoder::new(width, height, 30).expect("VideoToolbox encoder");
            encoder.force_keyframe().expect("force keyframe");
            encoder
                .encode_gpu_surface(surface)
                .expect("encode GPU surface");
            encoder.complete_frames();
            let sample = encoder
                .receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("receive GPU frame");
            assert!(sample.is_h264_idr());
        }
    }

    fn encode_video_frame(
        session: VTCompressionSessionRef,
        image_buffer: *mut c_void,
        pts: CMTime,
        duration: CMTime,
        timestamp: *mut c_void,
        force_keyframe: bool,
    ) -> Result<i32> {
        if !force_keyframe {
            return Ok(unsafe {
                VTCompressionSessionEncodeFrame(
                    session,
                    image_buffer,
                    pts,
                    duration,
                    ptr::null(),
                    timestamp,
                    ptr::null_mut(),
                )
            });
        }

        let keys = [unsafe { kVTEncodeFrameOptionKey_ForceKeyFrame }];
        let values = [unsafe { kCFBooleanTrue }];
        let properties = unsafe {
            CFDictionaryCreate(
                ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks).cast(),
                std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks).cast(),
            )
        };
        if properties.is_null() {
            return Err(video_toolbox_error(
                "failed to create force-keyframe properties",
                -1,
            ));
        }
        let status = unsafe {
            VTCompressionSessionEncodeFrame(
                session,
                image_buffer,
                pts,
                duration,
                properties,
                timestamp,
                ptr::null_mut(),
            )
        };
        unsafe { CFRelease(properties) };
        Ok(status)
    }

    fn set_bool_property(session: VTCompressionSessionRef, key: &str, value: bool) -> Result<()> {
        with_cf_string(key, |key| {
            let value = unsafe {
                if value {
                    kCFBooleanTrue
                } else {
                    kCFBooleanFalse
                }
            };
            let status = unsafe { VTSessionSetProperty(session, key, value.cast()) };
            if status == 0 {
                Ok(())
            } else {
                Err(video_toolbox_error(
                    "failed to set VideoToolbox bool property",
                    status,
                ))
            }
        })
    }

    fn set_i32_property(session: VTCompressionSessionRef, key: &str, value: i32) -> Result<()> {
        with_cf_string(key, |key| {
            let number = unsafe {
                CFNumberCreate(
                    ptr::null(),
                    K_CF_NUMBER_SINT32_TYPE,
                    (&value as *const i32).cast(),
                )
            };
            if number.is_null() {
                return Err(video_toolbox_error(
                    "failed to create CoreFoundation number",
                    -1,
                ));
            }
            let status = unsafe { VTSessionSetProperty(session, key, number.cast()) };
            unsafe { CFRelease(number.cast()) };
            if status == 0 {
                Ok(())
            } else {
                Err(video_toolbox_error(
                    "failed to set VideoToolbox integer property",
                    status,
                ))
            }
        })
    }

    fn set_string_property(session: VTCompressionSessionRef, key: &str, value: &str) -> Result<()> {
        with_cf_string(key, |key| {
            with_cf_string(value, |value| {
                let status = unsafe { VTSessionSetProperty(session, key, value.cast()) };
                if status == 0 {
                    Ok(())
                } else {
                    Err(video_toolbox_error(
                        "failed to set VideoToolbox string property",
                        status,
                    ))
                }
            })
        })
    }

    fn with_cf_string<T>(value: &str, f: impl FnOnce(*const c_void) -> Result<T>) -> Result<T> {
        let c_value = CString::new(value).map_err(|err| {
            Error::new(
                CaptureErrorCode::WebRtcTrackFailed,
                format!("failed to create CoreFoundation string: {err}"),
                true,
            )
        })?;
        let cf_string = unsafe {
            CFStringCreateWithCString(ptr::null(), c_value.as_ptr(), K_CF_STRING_ENCODING_UTF8)
        };
        if cf_string.is_null() {
            return Err(video_toolbox_error(
                "failed to create CoreFoundation string",
                -1,
            ));
        }
        let result = f(cf_string.cast());
        unsafe { CFRelease(cf_string.cast()) };
        result
    }

    type VTCompressionSessionRef = *mut c_void;
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_CF_NUMBER_SINT32_TYPE: i32 = 3;

    #[link(name = "VideoToolbox", kind = "framework")]
    #[link(name = "CoreMedia", kind = "framework")]
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn VTCompressionSessionCreate(
            allocator: *const c_void,
            width: i32,
            height: i32,
            codec_type: u32,
            encoder_specification: *const c_void,
            image_buffer_attributes: *const c_void,
            compressed_data_allocator: *const c_void,
            output_callback: Option<extern "C" fn(*mut c_void, *mut c_void, i32, u32, *mut c_void)>,
            output_callback_ref_con: *mut c_void,
            compression_session_out: *mut VTCompressionSessionRef,
        ) -> i32;

        fn VTCompressionSessionPrepareToEncodeFrames(session: VTCompressionSessionRef) -> i32;

        fn VTCompressionSessionEncodeFrame(
            session: VTCompressionSessionRef,
            image_buffer: *mut c_void,
            presentation_time_stamp: CMTime,
            duration: CMTime,
            frame_properties: *const c_void,
            source_frame_ref_con: *mut c_void,
            info_flags_out: *mut u32,
        ) -> i32;

        fn VTCompressionSessionCompleteFrames(
            session: VTCompressionSessionRef,
            complete_until_presentation_time_stamp: CMTime,
        ) -> i32;

        fn VTCompressionSessionInvalidate(session: VTCompressionSessionRef);

        fn VTSessionSetProperty(
            session: VTCompressionSessionRef,
            property_key: *const c_void,
            property_value: *const c_void,
        ) -> i32;

        fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            video_desc: *mut c_void,
            parameter_set_index: usize,
            parameter_set_pointer_out: *mut *const u8,
            parameter_set_size_out: *mut usize,
            parameter_set_count_out: *mut usize,
            nal_unit_header_length_out: *mut i32,
        ) -> i32;

        fn CFRelease(cf: *const c_void);

        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const i8,
            encoding: u32,
        ) -> *const c_void;

        fn CFNumberCreate(
            allocator: *const c_void,
            the_type: i32,
            value_ptr: *const c_void,
        ) -> *const c_void;

        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            count: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> *const c_void;

        static kCFBooleanTrue: *const c_void;
        static kCFBooleanFalse: *const c_void;
        static kCFTypeDictionaryKeyCallBacks: u8;
        static kCFTypeDictionaryValueCallBacks: u8;
        static kVTEncodeFrameOptionKey_ForceKeyFrame: *const c_void;
    }
}

#[cfg(target_os = "macos")]
pub(crate) use macos::H264Encoder;

#[cfg(target_os = "windows")]
#[allow(dead_code)]
mod windows {
    include!("h264_encoder_windows.rs");
}

#[cfg(target_os = "windows")]
pub use windows::H264Encoder;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub struct H264Encoder;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl H264Encoder {
    pub fn new(_width: u32, _height: u32, _fps: u32) -> crate::Result<Self> {
        Ok(Self)
    }

    pub fn encode_frame(
        &self,
        _frame: &crate::pipeline::frame::VideoFrame,
    ) -> crate::Result<Option<crate::webrtc::track::EncodedVideoSample>> {
        Ok(None)
    }

    pub fn force_keyframe(&mut self) -> crate::Result<()> {
        Ok(())
    }
}
