use std::{
    mem::ManuallyDrop,
    ptr, thread,
    time::{Duration, Instant},
};

use windows::{
    core::Interface,
    Win32::{
        Foundation::VARIANT_TRUE,
        Media::MediaFoundation::*,
        System::{
            Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED},
            Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL},
        },
    },
};
use yuvutils_rs::{bgra_to_yuv_nv12, YuvRange, YuvStandardMatrix};

use crate::{
    error::Error,
    models::{CaptureErrorCode, PixelFormat},
    pipeline::frame::VideoFrame,
    webrtc::track::EncodedVideoSample,
    Result,
};

pub(crate) struct MediaFoundationH264Encoder {
    transform: Option<IMFTransform>,
    event_generator: Option<IMFMediaEventGenerator>,
    width: u32,
    height: u32,
    frame_duration: Duration,
    frame_duration_hns: i64,
    com_initialized: bool,
}

unsafe impl Send for MediaFoundationH264Encoder {}

impl MediaFoundationH264Encoder {
    pub(crate) fn new(width: u32, height: u32, fps: u32) -> Result<Self> {
        if width % 2 != 0 || height % 2 != 0 {
            return Err(mf_error(format!(
                "Media Foundation H264 requires even dimensions, got {width}x{height}"
            )));
        }

        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(map_windows_error)?;
        if let Err(error) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) } {
            unsafe { CoUninitialize() };
            return Err(map_windows_error(error));
        }

        match create_hardware_transform(width, height, fps.max(1)) {
            Ok((transform, event_generator)) => Ok(Self {
                transform: Some(transform),
                event_generator,
                width,
                height,
                frame_duration: Duration::from_secs_f64(1.0 / f64::from(fps.max(1))),
                frame_duration_hns: 10_000_000 / i64::from(fps.max(1)),
                com_initialized: true,
            }),
            Err(error) => {
                let _ = unsafe { MFShutdown() };
                unsafe { CoUninitialize() };
                Err(error)
            }
        }
    }

    pub(crate) fn encode_frame(
        &mut self,
        frame: &VideoFrame,
    ) -> Result<Option<EncodedVideoSample>> {
        self.validate_frame(frame)?;
        if let Some(events) = &self.event_generator {
            wait_for_event(events, METransformNeedInput.0)?;
        }

        let sample = self.make_input_sample(frame)?;
        unsafe { self.transform().ProcessInput(0, &sample, 0) }.map_err(map_windows_error)?;
        if let Some(events) = &self.event_generator {
            wait_for_event(events, METransformHaveOutput.0)?;
        }

        let data = self.take_output()?;
        Ok(data.map(|data| EncodedVideoSample {
            data: normalize_h264(data),
            duration: self.frame_duration,
            timestamp_ns: frame.timestamp_ns,
        }))
    }

    pub(crate) fn force_keyframe(&mut self) -> Result<()> {
        let codec_api: ICodecAPI = self.transform().cast().map_err(map_windows_error)?;
        let value = VARIANT {
            Anonymous: VARIANT_0 {
                Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                    vt: VT_BOOL,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: VARIANT_0_0_0 {
                        boolVal: VARIANT_TRUE,
                    },
                }),
            },
        };
        unsafe { codec_api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &value) }
            .map_err(map_windows_error)
    }

    fn validate_frame(&self, frame: &VideoFrame) -> Result<()> {
        if frame.pixel_format != PixelFormat::Bgra
            || frame.width != self.width
            || frame.height != self.height
        {
            return Err(mf_error(format!(
                "Media Foundation expected {}x{} BGRA, got {}x{} {:?}",
                self.width, self.height, frame.width, frame.height, frame.pixel_format
            )));
        }
        Ok(())
    }

    fn transform(&self) -> &IMFTransform {
        self.transform.as_ref().expect("Media Foundation transform")
    }

    fn make_input_sample(&self, frame: &VideoFrame) -> Result<IMFSample> {
        let y_len = self.width as usize * self.height as usize;
        let mut nv12 = vec![0u8; y_len + y_len / 2];
        let (y, uv) = nv12.split_at_mut(y_len);
        bgra_to_yuv_nv12(
            y,
            self.width,
            uv,
            self.width,
            &frame.data,
            self.width * 4,
            self.width,
            self.height,
            YuvRange::TV,
            YuvStandardMatrix::Bt709,
        );

        let buffer =
            unsafe { MFCreateMemoryBuffer(nv12.len() as u32) }.map_err(map_windows_error)?;
        let mut target = ptr::null_mut();
        unsafe { buffer.Lock(&mut target, None, None) }.map_err(map_windows_error)?;
        unsafe { ptr::copy_nonoverlapping(nv12.as_ptr(), target, nv12.len()) };
        unsafe { buffer.Unlock() }.map_err(map_windows_error)?;
        unsafe { buffer.SetCurrentLength(nv12.len() as u32) }.map_err(map_windows_error)?;

        let sample = unsafe { MFCreateSample() }.map_err(map_windows_error)?;
        unsafe {
            sample.AddBuffer(&buffer).map_err(map_windows_error)?;
            sample
                .SetSampleTime((frame.timestamp_ns / 100) as i64)
                .map_err(map_windows_error)?;
            sample
                .SetSampleDuration(self.frame_duration_hns)
                .map_err(map_windows_error)?;
        }
        Ok(sample)
    }

    fn take_output(&self) -> Result<Option<Vec<u8>>> {
        let info = unsafe { self.transform().GetOutputStreamInfo(0) }.map_err(map_windows_error)?;
        let sample = unsafe { MFCreateSample() }.map_err(map_windows_error)?;
        let buffer = unsafe { MFCreateMemoryBuffer(info.cbSize.max(1024 * 1024)) }
            .map_err(map_windows_error)?;
        unsafe { sample.AddBuffer(&buffer) }.map_err(map_windows_error)?;

        let mut output = MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(Some(sample)),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        };
        let mut status = 0;
        let result = unsafe {
            self.transform()
                .ProcessOutput(0, std::slice::from_mut(&mut output), &mut status)
        };
        let output_sample = unsafe { ManuallyDrop::take(&mut output.pSample) };
        let _events = unsafe { ManuallyDrop::take(&mut output.pEvents) };

        if let Err(error) = result {
            if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT {
                return Ok(None);
            }
            return Err(map_windows_error(error));
        }

        let Some(sample) = output_sample else {
            return Ok(None);
        };
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }.map_err(map_windows_error)?;
        let mut bytes = ptr::null_mut();
        let mut len = 0;
        unsafe { buffer.Lock(&mut bytes, None, Some(&mut len)) }.map_err(map_windows_error)?;
        let data = unsafe { std::slice::from_raw_parts(bytes, len as usize) }.to_vec();
        unsafe { buffer.Unlock() }.map_err(map_windows_error)?;
        Ok((!data.is_empty()).then_some(data))
    }
}

impl Drop for MediaFoundationH264Encoder {
    fn drop(&mut self) {
        unsafe {
            if let Some(transform) = self.transform.as_ref() {
                let _ = transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
                let _ = transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            }
            self.event_generator.take();
            self.transform.take();
            let _ = MFShutdown();
            if self.com_initialized {
                CoUninitialize();
            }
        }
    }
}

fn create_hardware_transform(
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(IMFTransform, Option<IMFMediaEventGenerator>)> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut activates: *mut Option<IMFActivate> = ptr::null_mut();
    let mut count = 0;
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            &mut activates,
            &mut count,
        )
    }
    .map_err(map_windows_error)?;

    if activates.is_null() || count == 0 {
        return Err(mf_error("no native Windows hardware H264 MFT was found"));
    }

    let entries = unsafe { std::slice::from_raw_parts_mut(activates, count as usize) };
    let mut last_error = None;
    for index in 0..entries.len() {
        let Some(activate) = entries[index].take() else {
            continue;
        };
        let configured = unsafe { activate.ActivateObject::<IMFTransform>() }
            .map_err(map_windows_error)
            .and_then(|transform| configure_transform(transform, width, height, fps));
        match configured {
            Ok(transform) => {
                for remaining in entries.iter_mut() {
                    let _ = remaining.take();
                }
                unsafe { CoTaskMemFree(Some(activates.cast())) };
                return Ok(transform);
            }
            Err(error) => last_error = Some(error),
        }
    }
    unsafe { CoTaskMemFree(Some(activates.cast())) };
    Err(last_error.unwrap_or_else(|| mf_error("hardware H264 MFT activation failed")))
}

fn configure_transform(
    transform: IMFTransform,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(IMFTransform, Option<IMFMediaEventGenerator>)> {
    let attributes = unsafe { transform.GetAttributes() }.map_err(map_windows_error)?;
    let asynchronous =
        unsafe { attributes.GetUINT32(&MF_TRANSFORM_ASYNC).ok() }.is_some_and(|value| value != 0);
    let event_generator = if asynchronous {
        unsafe { attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1) }
            .map_err(map_windows_error)?;
        Some(
            transform
                .cast::<IMFMediaEventGenerator>()
                .map_err(map_windows_error)?,
        )
    } else {
        None
    };

    let output = unsafe { MFCreateMediaType() }.map_err(map_windows_error)?;
    let configure_output = (|| -> windows::core::Result<()> {
        unsafe {
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, recommended_bitrate(width, height, fps))?;
            output.SetUINT64(&MF_MT_FRAME_SIZE, pack_ratio(width, height))?;
            output.SetUINT64(&MF_MT_FRAME_RATE, pack_ratio(fps, 1))?;
            output.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)?;
            transform.SetOutputType(0, &output, 0)?;
        }
        Ok(())
    })();
    configure_output.map_err(map_windows_error)?;

    let input = unsafe { MFCreateMediaType() }.map_err(map_windows_error)?;
    let configure_input = (|| -> windows::core::Result<()> {
        unsafe {
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            input.SetUINT64(&MF_MT_FRAME_SIZE, pack_ratio(width, height))?;
            input.SetUINT64(&MF_MT_FRAME_RATE, pack_ratio(fps, 1))?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            transform.SetInputType(0, &input, 0)?;
        }
        Ok(())
    })();
    configure_input.map_err(map_windows_error)?;

    unsafe {
        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
            .map_err(map_windows_error)?;
        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
            .map_err(map_windows_error)?;
    }
    Ok((transform, event_generator))
}

fn wait_for_event(generator: &IMFMediaEventGenerator, expected: i32) -> Result<()> {
    const EVENT_TIMEOUT: Duration = Duration::from_secs(2);
    let deadline = Instant::now() + EVENT_TIMEOUT;
    loop {
        let event = match unsafe { generator.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
            Ok(event) => event,
            Err(error) if error.code() == MF_E_NO_EVENTS_AVAILABLE => {
                if Instant::now() >= deadline {
                    return Err(mf_error(format!(
                        "native Windows Media Foundation H264 timed out after {} ms waiting for event {expected}",
                        EVENT_TIMEOUT.as_millis()
                    )));
                }
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            Err(error) => return Err(map_windows_error(error)),
        };
        let kind = unsafe { event.GetType() }.map_err(map_windows_error)?;
        if kind == expected as u32 {
            return Ok(());
        }
    }
}

fn pack_ratio(numerator: u32, denominator: u32) -> u64 {
    (u64::from(numerator) << 32) | u64::from(denominator)
}

fn recommended_bitrate(width: u32, height: u32, fps: u32) -> u32 {
    let bits = u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(u64::from(fps))
        .saturating_mul(8)
        / 100;
    bits.clamp(4_000_000, 8_000_000) as u32
}

fn normalize_h264(data: Vec<u8>) -> Vec<u8> {
    if data.windows(3).any(|window| window == [0, 0, 1]) {
        return data;
    }
    let mut output = Vec::with_capacity(data.len() + 32);
    let mut offset = 0;
    while offset + 4 <= data.len() {
        let len = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        if len == 0 || offset + len > data.len() {
            return data;
        }
        output.extend_from_slice(&[0, 0, 0, 1]);
        output.extend_from_slice(&data[offset..offset + len]);
        offset += len;
    }
    if offset == data.len() && !output.is_empty() {
        output
    } else {
        data
    }
}

fn map_windows_error(error: windows::core::Error) -> Error {
    mf_error(format!(
        "native Windows Media Foundation H264 failed: {error}"
    ))
}

fn mf_error(message: impl Into<String>) -> Error {
    Error::new(CaptureErrorCode::WebRtcTrackFailed, message, true)
}

#[cfg(test)]
mod tests {
    use super::{normalize_h264, pack_ratio, recommended_bitrate};

    #[test]
    fn ratio_attributes_use_media_foundation_packing() {
        assert_eq!(pack_ratio(1920, 1080), 0x0000_0780_0000_0438);
    }

    #[test]
    fn avcc_output_is_normalized_to_annex_b() {
        assert_eq!(
            normalize_h264(vec![0, 0, 0, 2, 0x65, 0x88]),
            vec![0, 0, 0, 1, 0x65, 0x88]
        );
    }

    #[test]
    fn realtime_screen_share_bitrate_is_bounded_for_webrtc_backpressure() {
        assert_eq!(recommended_bitrate(1920, 1080, 60), 8_000_000);
        assert_eq!(recommended_bitrate(1280, 720, 30), 4_000_000);
    }
}
