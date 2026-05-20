#[cfg(target_os = "macos")]
mod macos {
    use std::{
        ffi::CString,
        ffi::c_void,
        ptr,
        sync::mpsc::{self, Receiver, Sender},
        time::Duration,
    };

    use apple_cf::{
        cm::{CMTime, CMSampleBuffer},
        cv::CVPixelBuffer,
    };

    use crate::{
        error::Error,
        models::{CaptureErrorCode, PixelFormat},
        pipeline::frame::VideoFrame,
        webrtc::track::EncodedVideoSample,
        Result,
    };

    const K_CM_VIDEO_CODEC_TYPE_H264: u32 = u32::from_be_bytes(*b"avc1");
    const K_CV_PIXEL_FORMAT_TYPE_32_BGRA: u32 = u32::from_be_bytes(*b"BGRA");

    pub struct H264Encoder {
        session: VTCompressionSessionRef,
        context: *mut EncoderContext,
        receiver: Receiver<EncodedVideoSample>,
        frame_duration: Duration,
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

            configure_session(session, width, height, fps)?;

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
            })
        }

        pub fn encode_frame(&self, frame: &VideoFrame) -> Result<Option<EncodedVideoSample>> {
            if frame.pixel_format != PixelFormat::Bgra {
                return Err(Error::new(
                    CaptureErrorCode::WebRtcTrackFailed,
                    "VideoToolbox H264 encoder currently expects BGRA frames",
                    true,
                ));
            }

            let mut data = frame.data.clone();
            let pixel_buffer = unsafe {
                CVPixelBuffer::create_with_bytes(
                    frame.width as usize,
                    frame.height as usize,
                    K_CV_PIXEL_FORMAT_TYPE_32_BGRA,
                    data.as_mut_ptr().cast(),
                    frame.width as usize * 4,
                )
            }
            .map_err(|status| video_toolbox_error("failed to create encoder pixel buffer", status))?;
            let timestamp = Box::into_raw(Box::new(frame.timestamp_ns));
            let pts = CMTime::new(
                i64::try_from(frame.timestamp_ns / 1_000).unwrap_or(i64::MAX),
                1_000_000,
            );
            let duration = CMTime::new(
                i64::try_from(self.frame_duration.as_micros()).unwrap_or(i64::MAX),
                1_000_000,
            );
            let status = unsafe {
                VTCompressionSessionEncodeFrame(
                    self.session,
                    pixel_buffer.as_ptr(),
                    pts,
                    duration,
                    ptr::null(),
                    timestamp.cast(),
                    ptr::null_mut(),
                )
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

            let _ = unsafe { VTCompressionSessionCompleteFrames(self.session, CMTime::INVALID) };
            Ok(self.receiver.try_iter().last())
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
        if status != 0 || output_callback_ref_con.is_null() || sample_buffer.is_null() {
            return;
        }

        let timestamp_ns = if source_frame_ref_con.is_null() {
            0
        } else {
            unsafe { *Box::from_raw(source_frame_ref_con.cast::<u64>()) }
        };
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

    fn configure_session(session: VTCompressionSessionRef, width: u32, height: u32, fps: u32) -> Result<()> {
        set_bool_property(session, "RealTime", true)?;
        set_i32_property(
            session,
            "AverageBitRate",
            recommended_bitrate(width, height, fps),
        )?;
        set_i32_property(session, "MaxKeyFrameInterval", i32::try_from(fps.max(1) * 2).unwrap_or(60))?;
        set_string_property(session, "ProfileLevel", "H264_Baseline_AutoLevel")?;
        Ok(())
    }

    fn recommended_bitrate(width: u32, height: u32, fps: u32) -> i32 {
        let pixels = u64::from(width).saturating_mul(u64::from(height));
        let fps = u64::from(fps.max(1));
        let bits = pixels
            .saturating_mul(fps)
            .saturating_mul(14)
            .saturating_div(100);
        i32::try_from(bits.clamp(4_000_000, 24_000_000)).unwrap_or(12_000_000)
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
                Err(video_toolbox_error("failed to set VideoToolbox bool property", status))
            }
        })
    }

    fn set_i32_property(session: VTCompressionSessionRef, key: &str, value: i32) -> Result<()> {
        with_cf_string(key, |key| {
            let number = unsafe { CFNumberCreate(ptr::null(), K_CF_NUMBER_SINT32_TYPE, (&value as *const i32).cast()) };
            if number.is_null() {
                return Err(video_toolbox_error("failed to create CoreFoundation number", -1));
            }
            let status = unsafe { VTSessionSetProperty(session, key, number.cast()) };
            unsafe { CFRelease(number.cast()) };
            if status == 0 {
                Ok(())
            } else {
                Err(video_toolbox_error("failed to set VideoToolbox integer property", status))
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
                    Err(video_toolbox_error("failed to set VideoToolbox string property", status))
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
            return Err(video_toolbox_error("failed to create CoreFoundation string", -1));
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
            output_callback: Option<
                extern "C" fn(*mut c_void, *mut c_void, i32, u32, *mut c_void),
            >,
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

        static kCFBooleanTrue: *const c_void;
        static kCFBooleanFalse: *const c_void;
    }
}

#[cfg(target_os = "macos")]
pub use macos::H264Encoder;

#[cfg(not(target_os = "macos"))]
pub struct H264Encoder;

#[cfg(not(target_os = "macos"))]
impl H264Encoder {
    pub fn encode_frame(
        &self,
        _frame: &crate::pipeline::frame::VideoFrame,
    ) -> crate::Result<Option<crate::webrtc::track::EncodedVideoSample>> {
        Ok(None)
    }
}
