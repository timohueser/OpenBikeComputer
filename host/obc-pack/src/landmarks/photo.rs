use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Rgb, RgbImage};
use std::io::Cursor;

pub const WIDTH: u32 = 216;
pub const HEIGHT: u32 = 240;
pub const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_DIMENSION: u32 = 16384;
const BAYER: [[u32; 4]; 4] = [[1, 9, 3, 11], [13, 5, 15, 7], [4, 12, 2, 10], [16, 8, 14, 6]];

/// Full source image, orientation applied, Lanczos fit, white padding, fixed RGB222 ordered dither.
pub fn prepare(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err("image_bytes");
    }
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|_| "image_format")?;
    if !matches!(reader.format(), Some(ImageFormat::Jpeg | ImageFormat::Png)) {
        return Err("image_format");
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| "image_decode")?;
    let orientation = decoder.orientation().map_err(|_| "image_orientation")?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|_| "image_decode")?;
    image.apply_orientation(orientation);
    let scaled = image.resize(WIDTH, HEIGHT, image::imageops::FilterType::Lanczos3).to_rgba8();
    let mut canvas = RgbImage::from_pixel(WIDTH, HEIGHT, Rgb([255, 255, 255]));
    let left = (WIDTH - scaled.width()) / 2;
    let top = (HEIGHT - scaled.height()) / 2;
    for (x, y, pixel) in scaled.enumerate_pixels() {
        let alpha = u32::from(pixel[3]);
        canvas.put_pixel(
            x + left,
            y + top,
            Rgb(core::array::from_fn(|c| ((u32::from(pixel[c]) * alpha + 255 * (255 - alpha) + 127) / 255) as u8)),
        );
    }
    Ok(canvas
        .enumerate_pixels()
        .map(|(x, y, pixel)| {
            let quantize = |channel: u8| {
                let scaled = u32::from(channel) * 3;
                let lower = scaled / 255;
                let remainder = scaled % 255;
                (lower + u32::from(remainder * 17 > BAYER[y as usize % 4][x as usize % 4] * 255)).min(3) as u8
            };
            quantize(pixel[0]) << 4 | quantize(pixel[1]) << 2 | quantize(pixel[2])
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_image_fits_without_crop_and_pixels_are_rgb222() {
        let source = DynamicImage::ImageRgb8(RgbImage::from_pixel(20, 10, Rgb([255, 0, 0])));
        let mut encoded = Cursor::new(Vec::new());
        source.write_to(&mut encoded, ImageFormat::Png).unwrap();
        let pixels = prepare(encoded.get_ref()).unwrap();
        assert_eq!(pixels.len(), (WIDTH * HEIGHT) as usize);
        assert!(pixels.iter().all(|&p| p < 64));
        assert_eq!(pixels[0], 63);
        assert_eq!(pixels[(WIDTH * HEIGHT / 2) as usize], 48);
        assert_eq!(prepare(b"corrupt"), Err("image_format"));
    }
}
