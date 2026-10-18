//! Images for the model: decoded with WIC, scaled to fit 1,568 pixels on the long side (more
//! costs tokens without helping a note), and sent as PNG, or as they are when they already fit
//! and are a format the APIs take. Base64 for the request body.

use crate::platform::png::{ComScope, Pixels, encode_png};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatGif, GUID_ContainerFormatJpeg,
    GUID_ContainerFormatPng, GUID_ContainerFormatWebp, GUID_WICPixelFormat32bppBGRA,
    IWICBitmapSource, IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant,
    WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};

/// The longest side an image is sent at.
pub const MAX_EDGE: u32 = 1568;
/// At most this many images go in one request.
pub const MAX_IMAGES: usize = 8;

/// An image ready for a request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelImage {
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
    pub media_type: &'static str,
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl ModelImage {
    pub fn base64(&self) -> String {
        base64(&self.data)
    }
}

/// The size that fits `width` × `height` inside `MAX_EDGE`, keeping its shape.
pub fn fitted(width: u32, height: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= MAX_EDGE {
        return (width, height);
    }
    let scale = |side: u32| {
        ((u64::from(side) * u64::from(MAX_EDGE) + u64::from(long) / 2) / u64::from(long)).max(1)
            as u32
    };
    (scale(width), scale(height))
}

/// `bytes` (any format WIC reads) prepared for the model.
pub fn prepare(bytes: &[u8]) -> Result<ModelImage, String> {
    let _com = ComScope::enter();
    let fail =
        |error: windows::core::Error| format!("the image couldn't be read: {}", error.message());
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(fail)?;
        let stream = factory.CreateStream().map_err(fail)?;
        stream.InitializeFromMemory(bytes).map_err(fail)?;
        let decoder = factory
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
            .map_err(fail)?;
        let container = decoder.GetContainerFormat().map_err(fail)?;
        let frame = decoder.GetFrame(0).map_err(fail)?;
        let (mut width, mut height) = (0u32, 0u32);
        frame.GetSize(&mut width, &mut height).map_err(fail)?;
        let (fit_width, fit_height) = fitted(width, height);
        let kept_as_is = [
            (GUID_ContainerFormatPng, "image/png"),
            (GUID_ContainerFormatJpeg, "image/jpeg"),
            (GUID_ContainerFormatGif, "image/gif"),
            (GUID_ContainerFormatWebp, "image/webp"),
        ]
        .into_iter()
        .find(|(format, _)| *format == container)
        .map(|(_, media_type)| media_type);
        if let Some(media_type) = kept_as_is
            && (fit_width, fit_height) == (width, height)
        {
            return Ok(ModelImage {
                media_type,
                data: bytes.to_vec(),
                width,
                height,
            });
        }
        let converter = factory.CreateFormatConverter().map_err(fail)?;
        converter
            .Initialize(
                &frame,
                &GUID_WICPixelFormat32bppBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(fail)?;
        let source: IWICBitmapSource = if (fit_width, fit_height) == (width, height) {
            converter.into()
        } else {
            let scaler = factory.CreateBitmapScaler().map_err(fail)?;
            scaler
                .Initialize(
                    &converter,
                    fit_width,
                    fit_height,
                    WICBitmapInterpolationModeFant,
                )
                .map_err(fail)?;
            scaler.into()
        };
        let stride = fit_width * 4;
        let mut bgra = vec![0u8; (stride * fit_height) as usize];
        source
            .CopyPixels(std::ptr::null(), stride, &mut bgra)
            .map_err(fail)?;
        let png = encode_png(&Pixels {
            width: fit_width,
            height: fit_height,
            bgra,
        })?;
        Ok(ModelImage {
            media_type: "image/png",
            data: png,
            width: fit_width,
            height: fit_height,
        })
    }
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for index in 0..width * height {
            bgra.extend_from_slice(&[(index % 251) as u8, 0, 0, 255]);
        }
        encode_png(&Pixels {
            width,
            height,
            bgra,
        })
        .unwrap()
    }

    #[test]
    fn sizes_fit_the_long_edge() {
        assert_eq!(fitted(1000, 800), (1000, 800));
        assert_eq!(fitted(4000, 3000), (1568, 1176));
        assert_eq!(fitted(3000, 4000), (1176, 1568));
        assert_eq!(fitted(1568, 10), (1568, 10));
        assert_eq!(fitted(100_000, 1), (1568, 1));
    }

    #[test]
    fn a_small_png_goes_as_it_is_and_a_large_one_is_scaled() {
        let small = png(40, 30);
        let prepared = prepare(&small).unwrap();
        assert_eq!(
            (prepared.media_type, prepared.width, prepared.height),
            ("image/png", 40, 30)
        );
        assert_eq!(prepared.data, small);

        let large = prepare(&png(4000, 3000)).unwrap();
        assert_eq!(
            (large.media_type, large.width, large.height),
            ("image/png", 1568, 1176)
        );
        assert_eq!(
            &large.data[16..24],
            &[0, 0, 6, 32, 0, 0, 4, 152],
            "IHDR says 1568 × 1176"
        );
    }

    #[test]
    fn a_bmp_is_sent_as_png_and_junk_is_refused() {
        // A 1 × 1 24-bit BMP.
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&58u32.to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        for value in [40u32, 1, 1] {
            bmp.extend_from_slice(&value.to_le_bytes());
        }
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&24u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 24]);
        bmp.extend_from_slice(&[0, 0, 255, 0]);
        let prepared = prepare(&bmp).unwrap();
        assert_eq!(
            (prepared.media_type, prepared.width, prepared.height),
            ("image/png", 1, 1)
        );
        assert!(prepare(b"not an image").is_err());
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xFF, 0xFE, 0xFD]), "//79");
    }
}
