use std::{ffi::OsStr, os::windows::ffi::OsStrExt};

use windows::core::Error as WindowsError;

use crate::{error::Error, models::CaptureErrorCode};

pub(crate) fn rgb_to_colorref(color: u32) -> u32 {
    ((color & 0x0000_00ff) << 16) | (color & 0x0000_ff00) | ((color & 0x00ff_0000) >> 16)
}

pub(crate) fn windows_error(context: &str, error: WindowsError) -> Error {
    Error::new(
        CaptureErrorCode::Internal,
        format!("{context}: {error}"),
        true,
    )
}

pub(crate) fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
