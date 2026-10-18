//! Copied from FastPad `src/platform/png.rs`.
//!
//! Clipboard bitmaps as PNG files: a `CF_DIB` or `CF_DIBV5` block read into top-down BGRA
//! pixels, then encoded with WIC. WIC is only loaded here, when an image is captured.

use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA,
    IWICBitmapFrameEncode, IWICImagingFactory, WICBitmapEncoderNoCache,
};
use windows::Win32::System::Com::StructuredStorage::{
    CreateStreamOnHGlobal, GetHGlobalFromStream, IPropertyBag2,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};

/// COM for this thread while it lives, when this call started it.
pub(crate) struct ComScope(bool);

impl ComScope {
    pub(crate) fn enter() -> Self {
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

/// Pixels ready to encode: 32-bit BGRA, top row first, `width * 4` bytes a row.
#[derive(Debug, Eq, PartialEq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Reads a packed DIB (a `BITMAPINFOHEADER`, `BITMAPV4HEADER` or `BITMAPV5HEADER`, its masks
/// and the pixels) of 24 or 32 bits a pixel. Other depths and compressed DIBs give `None`.
/// 32-bit pixels whose alpha is all zero (as most apps leave it) are made opaque.
pub fn dib_to_pixels(dib: &[u8]) -> Option<Pixels> {
    let header = u32_at(dib, 0)? as usize;
    if header < 40 {
        return None;
    }
    let width = i32::from_le_bytes(dib.get(4..8)?.try_into().ok()?);
    let height = i32::from_le_bytes(dib.get(8..12)?.try_into().ok()?);
    let bits = u16_at(dib, 14)?;
    let compression = u32_at(dib, 16)?;
    let colors_used = u32_at(dib, 32)? as usize;
    if width <= 0 || height == 0 || !matches!(bits, 24 | 32) {
        return None;
    }
    if compression != BI_RGB && !(compression == BI_BITFIELDS && bits == 32) {
        return None;
    }
    // A plain info header is followed by its three masks; later headers hold them.
    let masks = if compression == BI_BITFIELDS && header == 40 {
        12
    } else {
        0
    };
    let offset = header + masks + colors_used * 4;
    let (width, rows) = (width as usize, height.unsigned_abs() as usize);
    let stride = (width * usize::from(bits)).div_ceil(32) * 4;
    let data = dib.get(offset..offset + stride * rows)?;
    let mut bgra = Vec::with_capacity(width * rows * 4);
    for row in 0..rows {
        // A positive height means the bottom row comes first.
        let source = if height > 0 { rows - 1 - row } else { row };
        let line = &data[source * stride..source * stride + stride];
        for x in 0..width {
            if bits == 32 {
                bgra.extend_from_slice(&line[x * 4..x * 4 + 4]);
            } else {
                bgra.extend_from_slice(&line[x * 3..x * 3 + 3]);
                bgra.push(255);
            }
        }
    }
    if bits == 32 && bgra.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 0) {
        for pixel in bgra.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
    }
    Some(Pixels {
        width: width as u32,
        height: rows as u32,
        bgra,
    })
}

/// `pixels` as a PNG file's bytes.
pub fn encode_png(pixels: &Pixels) -> Result<Vec<u8>, String> {
    let _com = ComScope::enter();
    let fail = |error: windows::core::Error| {
        format!(
            "FastCapture could not encode the image: {}",
            error.message()
        )
    };
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(fail)?;
        let stream = CreateStreamOnHGlobal(Default::default(), true).map_err(fail)?;
        let encoder = factory
            .CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())
            .map_err(fail)?;
        encoder
            .Initialize(&stream, WICBitmapEncoderNoCache)
            .map_err(fail)?;
        let mut frame: Option<IWICBitmapFrameEncode> = None;
        let mut options: Option<IPropertyBag2> = None;
        encoder
            .CreateNewFrame(&mut frame, &mut options)
            .map_err(fail)?;
        let frame = frame.ok_or_else(|| "FastCapture could not encode the image".to_owned())?;
        frame.Initialize(options.as_ref()).map_err(fail)?;
        frame.SetSize(pixels.width, pixels.height).map_err(fail)?;
        let mut format = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut format).map_err(fail)?;
        frame
            .WritePixels(pixels.height, pixels.width * 4, &pixels.bgra)
            .map_err(fail)?;
        frame.Commit().map_err(fail)?;
        encoder.Commit().map_err(fail)?;
        let memory = GetHGlobalFromStream(&stream).map_err(fail)?;
        let handle = memory.0;
        let size = windows_sys::Win32::System::Memory::GlobalSize(handle);
        let data = windows_sys::Win32::System::Memory::GlobalLock(handle).cast::<u8>();
        if data.is_null() {
            return Err("FastCapture could not encode the image".to_owned());
        }
        // The stream's size is what was written; GlobalSize may round up.
        let written = {
            use windows::Win32::System::Com::STREAM_SEEK_CUR;
            let mut position = 0_u64;
            stream
                .Seek(0, STREAM_SEEK_CUR, Some(&mut position))
                .map_err(fail)?;
            (position as usize).min(size)
        };
        let bytes = std::slice::from_raw_parts(data, written).to_vec();
        windows_sys::Win32::System::Memory::GlobalUnlock(handle);
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dib(bits: u16, height: i32, rows: &[&[u8]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&40_u32.to_le_bytes());
        bytes.extend_from_slice(&2_i32.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes.extend_from_slice(&[0; 24]);
        for row in rows {
            bytes.extend_from_slice(row);
        }
        bytes
    }

    #[test]
    fn a_bottom_up_24_bit_dib_reads_top_row_first_and_opaque() {
        // Two pixels a row: 6 bytes, padded to 8.
        let bottom: &[u8] = &[1, 2, 3, 4, 5, 6, 0, 0];
        let top: &[u8] = &[7, 8, 9, 10, 11, 12, 0, 0];
        let pixels = dib_to_pixels(&dib(24, 2, &[bottom, top])).unwrap();
        assert_eq!((pixels.width, pixels.height), (2, 2));
        assert_eq!(
            pixels.bgra,
            [7, 8, 9, 255, 10, 11, 12, 255, 1, 2, 3, 255, 4, 5, 6, 255]
        );
    }

    #[test]
    fn a_32_bit_dib_with_no_alpha_becomes_opaque_and_keeps_real_alpha() {
        let row: &[u8] = &[1, 2, 3, 0, 4, 5, 6, 0];
        let pixels = dib_to_pixels(&dib(32, -1, &[row])).unwrap();
        assert_eq!(pixels.bgra, [1, 2, 3, 255, 4, 5, 6, 255]);
        let row: &[u8] = &[1, 2, 3, 128, 4, 5, 6, 0];
        let pixels = dib_to_pixels(&dib(32, -1, &[row])).unwrap();
        assert_eq!(pixels.bgra, [1, 2, 3, 128, 4, 5, 6, 0]);
    }

    #[test]
    fn unsupported_or_short_dibs_are_refused() {
        assert_eq!(dib_to_pixels(&dib(8, 1, &[&[0; 4]])), None);
        assert_eq!(
            dib_to_pixels(&dib(24, 2, &[&[0; 8]])),
            None,
            "a row is missing"
        );
        assert_eq!(dib_to_pixels(&[0; 10]), None);
    }

    #[test]
    fn pixels_encode_as_a_png_of_the_same_size() {
        let pixels = Pixels {
            width: 2,
            height: 1,
            bgra: vec![255, 0, 0, 255, 0, 0, 255, 255],
        };
        let png = encode_png(&pixels).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        // The IHDR chunk comes first: its width and height, big-endian.
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..24], &[0, 0, 0, 2, 0, 0, 0, 1]);
    }
}
