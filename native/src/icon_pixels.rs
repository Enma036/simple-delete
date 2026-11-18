//! Local icon decoding only: no target executable is ever launched.
use eframe::egui::ColorImage;
use std::{
    fs,
    io::{Cursor, Read},
    os::windows::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path, Prefix},
};
use windows_sys::Win32::{
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
        DeleteDC, DeleteObject, GdiFlush, HBITMAP, HDC, HGDIOBJ, SelectObject,
    },
    Storage::FileSystem::GetDriveTypeW,
    UI::{
        Shell::ExtractIconExW,
        WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx, HICON, PrivateExtractIconsW},
    },
};

const SIDE: usize = 64;
const MAX_PNG_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PIXELS: u64 = 4 * 1024 * 1024;
const REPARSE: u32 = 0x400;

/// Accept only ordinary absolute DOS paths. UNC/device paths, ADS and parent traversal are excluded.
fn local_path_syntax(path: &Path) -> bool {
    let text = path.to_string_lossy();
    if text.contains('\0')
        || text.starts_with('\\')
        || text.starts_with('/')
        || text.chars().skip(2).any(|c| c == ':')
    {
        return false;
    }
    let mut parts = path.components();
    if !matches!(parts.next(),Some(Component::Prefix(prefix)) if matches!(prefix.kind(),Prefix::Disk(_)))
        || !matches!(parts.next(), Some(Component::RootDir))
    {
        return false;
    }
    parts.all(|part| matches!(part, Component::Normal(_)))
}

/// An existing local regular file or directory with no reparse-point ancestors.
pub fn is_local_path(path: &Path) -> bool {
    if !local_path_syntax(path) {
        return false;
    }
    let Some(root) = path.ancestors().last() else {
        return false;
    };
    let wide: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
    // 2=removable, 3=fixed, 5=CD-ROM, 6=RAM disk. Reject mapped remote and missing drives.
    if !matches!(unsafe { GetDriveTypeW(wide.as_ptr()) }, 2 | 3 | 5 | 6) {
        return false;
    }
    for ancestor in path.ancestors() {
        let Ok(meta) = fs::symlink_metadata(ancestor) else {
            return false;
        };
        if meta.file_type().is_symlink() || meta.file_attributes() & REPARSE != 0 {
            return false;
        }
        if ancestor == path && !meta.is_file() && !meta.is_dir() {
            return false;
        }
    }
    true
}

pub fn load(path: &Path, index: i32) -> Option<ColorImage> {
    if !is_local_path(path) || !fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => load_png(path),
        "exe" | "dll" | "ico" | "icl" => {
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let icon = extract(&wide, index)?;
            let mut surface = Dib::new()?;
            // Reconstruct coverage from black and white renders. This also supports legacy
            // AND-mask icons whose bitmap alpha channel is entirely zero.
            let black = surface.draw(icon.0, 0)?;
            let white = surface.draw(icon.0, 255)?;
            let rgba = composite_to_rgba(&black, &white)?;
            Some(ColorImage::from_rgba_unmultiplied([SIDE, SIDE], &rgba))
        }
        _ => None,
    }
}

fn load_png(path: &Path) -> Option<ColorImage> {
    if fs::metadata(path).ok()?.len() > MAX_PNG_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_PNG_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_PNG_BYTES {
        return None;
    }
    let reader =
        || image::ImageReader::with_format(Cursor::new(bytes.as_slice()), image::ImageFormat::Png);
    let (width, height) = reader().into_dimensions().ok()?;
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || width as u64 * height as u64 > MAX_PIXELS
    {
        return None;
    }
    let mut decoder = reader();
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(16 * 1024 * 1024);
    decoder.limits(limits);
    let image = decoder
        .decode()
        .ok()?
        .thumbnail(SIDE as u32, SIDE as u32)
        .into_rgba8();
    Some(ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}

struct OwnedIcon(HICON);
impl Drop for OwnedIcon {
    fn drop(&mut self) {
        unsafe {
            DestroyIcon(self.0);
        }
    }
}

fn extract(path: &[u16], index: i32) -> Option<OwnedIcon> {
    let mut handle = std::ptr::null_mut();
    let result = unsafe {
        PrivateExtractIconsW(
            path.as_ptr(),
            index,
            SIDE as i32,
            SIDE as i32,
            &mut handle,
            std::ptr::null_mut(),
            1,
            0,
        )
    };
    if !handle.is_null() {
        let owned = OwnedIcon(handle);
        if result == 1 {
            return Some(owned);
        }
        drop(owned);
    }
    handle = std::ptr::null_mut();
    let result =
        unsafe { ExtractIconExW(path.as_ptr(), index, &mut handle, std::ptr::null_mut(), 1) };
    if handle.is_null() {
        return None;
    }
    let owned = OwnedIcon(handle);
    (result == 1).then_some(owned)
}

struct Dib {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
}
impl Dib {
    fn new() -> Option<Self> {
        let dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
        if dc.is_null() {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIDE as i32,
                biHeight: -(SIDE as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = unsafe {
            CreateDIBSection(
                dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            )
        };
        if bitmap.is_null() || bits.is_null() {
            unsafe {
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
                DeleteDC(dc);
            }
            return None;
        }
        let previous = unsafe { SelectObject(dc, bitmap) };
        if previous.is_null() || previous as isize == -1 {
            unsafe {
                DeleteObject(bitmap);
                DeleteDC(dc);
            }
            return None;
        }
        Some(Self {
            dc,
            bitmap,
            previous,
            bits: bits.cast(),
        })
    }
    fn draw(&mut self, icon: HICON, background: u8) -> Option<Vec<u8>> {
        // No pending GDI work may access the DIB while its bits are written/read directly.
        unsafe {
            GdiFlush();
        }
        let bytes = unsafe { std::slice::from_raw_parts_mut(self.bits, SIDE * SIDE * 4) };
        for pixel in bytes.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[background, background, background, 255]);
        }
        let ok = unsafe {
            DrawIconEx(
                self.dc,
                0,
                0,
                icon,
                SIDE as i32,
                SIDE as i32,
                0,
                std::ptr::null_mut(),
                DI_NORMAL,
            )
        };
        unsafe {
            GdiFlush();
        }
        if ok == 0 {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts(self.bits, SIDE * SIDE * 4) }.to_vec())
    }
}
impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            GdiFlush();
            SelectObject(self.dc, self.previous);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

/// GDI produces premultiplied BGRA over the given backgrounds; recover straight RGBA.
fn composite_to_rgba(black: &[u8], white: &[u8]) -> Option<Vec<u8>> {
    if black.len() != white.len() || black.len() % 4 != 0 {
        return None;
    }
    let mut rgba = Vec::with_capacity(black.len());
    for (b, w) in black.chunks_exact(4).zip(white.chunks_exact(4)) {
        let transparency = (0..3).map(|i| w[i].saturating_sub(b[i])).max().unwrap();
        let alpha = 255 - transparency;
        if alpha == 0 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let straight =
            |channel: u8| ((channel as u32 * 255 + alpha as u32 / 2) / alpha as u32).min(255) as u8;
        rgba.extend_from_slice(&[straight(b[2]), straight(b[1]), straight(b[0]), alpha]);
    }
    Some(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconstructs_transparent_and_translucent_bgra() {
        assert_eq!(
            composite_to_rgba(&[0, 0, 0, 0], &[255, 255, 255, 0]).unwrap(),
            [0, 0, 0, 0]
        );
        assert_eq!(
            composite_to_rgba(&[0, 0, 128, 128], &[127, 127, 255, 128]).unwrap(),
            [255, 0, 0, 128]
        );
        assert_eq!(
            composite_to_rgba(&[30, 20, 10, 255], &[30, 20, 10, 255]).unwrap(),
            [10, 20, 30, 255]
        );
    }
    #[test]
    fn zero_alpha_legacy_mask_keeps_opaque_black_and_white_pixels() {
        assert_eq!(
            composite_to_rgba(&[0, 0, 0, 0], &[0, 0, 0, 0]).unwrap(),
            [0, 0, 0, 255]
        );
        assert_eq!(
            composite_to_rgba(&[255, 255, 255, 0], &[255, 255, 255, 0]).unwrap(),
            [255, 255, 255, 255]
        );
        assert!(composite_to_rgba(&[0], &[0]).is_none());
    }
    #[test]
    fn rejects_network_device_relative_ads_and_traversal_paths() {
        for path in [
            r"\\server\share\icon.ico",
            r"\\?\C:\icon.ico",
            r"\\.\C:\icon.ico",
            r"C:icon.ico",
            r"icon.ico",
            r"C:\icon.ico:secret",
            r"C:\Apps\..\icon.ico",
            "https://example.com/icon.png",
        ] {
            assert!(!local_path_syntax(Path::new(path)), "{path}");
        }
        assert!(local_path_syntax(Path::new(
            r"C:\Program Files\App\app.exe"
        )));
        assert!(local_path_syntax(Path::new(r"D:\Games\icon.ico")));
    }
}
