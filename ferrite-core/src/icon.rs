//! Extracts a process's own exe icon as raw RGBA pixels, for the GUI's
//! process picker. Pure Win32 — no GUI dependency, matching this crate's
//! "no GUI deps" rule; turning `IconRgba` into a texture is `ferrite-gui`'s
//! job.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, HBITMAP, ReleaseDC,
};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON, SHGetFileInfoW};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
use windows::core::PCWSTR;

/// A decoded icon: `rgba.len() == width * height * 4`, row-major, top-down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconRgba {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Extracts the small (list-view-sized) icon Windows Explorer would show
/// for `path`. Returns `None` for a missing/inaccessible path, or any step
/// of the Win32 pipeline failing — this is decoration, not a hard
/// requirement, so a caller should treat `None` as "no icon" rather than
/// an error to surface.
pub fn extract_icon_rgba(path: &Path) -> Option<IconRgba> {
    let wide_path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut file_info = SHFILEINFOW::default();
    // SAFETY: `wide_path` is a valid, null-terminated wide string for the
    // duration of this call; `file_info` is a valid, correctly-sized
    // out-parameter.
    let result = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide_path.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&raw mut file_info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_SMALLICON,
        )
    };
    if result == 0 {
        return None;
    }
    let hicon = file_info.hIcon;
    // Every early-return past this point must still destroy `hicon` - do
    // the rest of the work in a closure so `?`/early exits can't leak it.
    let icon = extract_from_hicon(hicon);
    // SAFETY: `hicon` came from the successful SHGetFileInfoW call above
    // and hasn't been destroyed yet.
    let _ = unsafe { DestroyIcon(hicon) };
    icon
}

fn extract_from_hicon(hicon: HICON) -> Option<IconRgba> {
    let mut icon_info = ICONINFO::default();
    // SAFETY: `hicon` is a valid icon handle; `icon_info` is a valid,
    // correctly-sized out-parameter.
    unsafe { GetIconInfo(hicon, &raw mut icon_info) }.ok()?;

    // hbmMask is only needed for GetIconInfo's own bookkeeping - we read
    // color+alpha from hbmColor alone (Windows' 32bpp icon bitmaps carry a
    // real alpha channel), so free the mask immediately rather than carry
    // it any further.
    // SAFETY: `hbmMask` came from the successful GetIconInfo call above.
    let _ = unsafe { DeleteObject(icon_info.hbmMask.into()) };

    let result = extract_from_hbitmap(icon_info.hbmColor);
    // SAFETY: `hbmColor` came from the successful GetIconInfo call above.
    let _ = unsafe { DeleteObject(icon_info.hbmColor.into()) };
    result
}

fn extract_from_hbitmap(hbitmap: HBITMAP) -> Option<IconRgba> {
    let mut bitmap = BITMAP::default();
    // SAFETY: `hbitmap` is a valid bitmap handle; `bitmap` is a valid,
    // correctly-sized out-parameter.
    let written = unsafe {
        GetObjectW(
            hbitmap.into(),
            size_of::<BITMAP>() as i32,
            Some(&raw mut bitmap as *mut _ as *mut core::ffi::c_void),
        )
    };
    if written == 0 || bitmap.bmWidth <= 0 || bitmap.bmHeight <= 0 {
        return None;
    }
    let width = bitmap.bmWidth as u32;
    let height = bitmap.bmHeight as u32;

    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = bitmap.bmWidth;
    info.bmiHeader.biHeight = -bitmap.bmHeight; // negative: top-down rows
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB.0;

    let mut buffer = vec![0u8; (width * height * 4) as usize];
    // SAFETY: `hdc` is a valid screen device context; `hbitmap` is a valid
    // bitmap handle; `buffer` is a valid, uniquely-owned allocation exactly
    // matching `width * height * 4` bytes, matching `info`'s header.
    let hdc = unsafe { GetDC(None) };
    let lines = unsafe {
        GetDIBits(
            hdc,
            hbitmap,
            0,
            height,
            Some(buffer.as_mut_ptr().cast::<core::ffi::c_void>()),
            &raw mut info,
            DIB_RGB_COLORS,
        )
    };
    // SAFETY: `hdc` came from the `GetDC(None)` call directly above.
    unsafe {
        ReleaseDC(None, hdc);
    }
    if lines == 0 {
        return None;
    }

    // GetDIBits returns BGRA; egui::ColorImage::from_rgba_unmultiplied
    // expects RGBA - swap B and R per pixel.
    for pixel in buffer.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    Some(IconRgba {
        width,
        height,
        rgba: buffer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_a_real_icon_from_a_known_system_binary() {
        // notepad.exe ships with every Windows install, at this exact path
        // on every supported architecture's default install.
        let path = Path::new(r"C:\Windows\System32\notepad.exe");
        let icon = extract_icon_rgba(path).expect("extracting notepad.exe's icon");

        assert!(icon.width > 0 && icon.height > 0);
        assert_eq!(
            icon.rgba.len(),
            (icon.width * icon.height * 4) as usize,
            "rgba buffer must be exactly width * height * 4 bytes"
        );
    }

    #[test]
    fn a_nonexistent_path_returns_none_not_a_panic() {
        let path = Path::new(r"C:\definitely\not\a\real\path\ferrite-test.exe");
        assert_eq!(extract_icon_rgba(path), None);
    }
}
