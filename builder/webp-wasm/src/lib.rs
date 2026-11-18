//! The RGBA bytes of a WebP image as stored, with the colour under every alpha. A browser that decodes images
//! premultiplied (WebKit) loses the colour under a low alpha, which TrailView's ground tiles carry.

/// The RGBA bytes of a WebP image, row by row; an image without alpha is opaque.
pub fn decode(body: &[u8]) -> Result<Vec<u8>, image_webp::DecodingError> {
    let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(body))?;
    let mut pixels = vec![0; decoder.output_buffer_size().ok_or(image_webp::DecodingError::ImageTooLarge)?];
    decoder.read_image(&mut pixels)?;
    if decoder.has_alpha() {
        return Ok(pixels);
    }
    Ok(pixels.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect())
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn decode_webp(body: &[u8]) -> Result<Vec<u8>, wasm_bindgen::JsError> {
    decode(body).map_err(|error| wasm_bindgen::JsError::new(&error.to_string()))
}

#[cfg(test)]
mod tests {
    // The probe of the ground worker: (200, 100, 50) under alpha 0, 1, 2 and 255, lossless.
    const PROBE: &[u8] = &[
        0x52, 0x49, 0x46, 0x46, 0x2a, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50, 0x56, 0x50, 0x38, 0x4c, 0x1e, 0x00,
        0x00, 0x00, 0x2f, 0x01, 0x40, 0x00, 0x10, 0x1f, 0x30, 0x64, 0x83, 0x3c, 0x32, 0x02, 0x92, 0x24, 0xff, 0x6f,
        0x0e, 0xe2, 0x01, 0x60, 0x90, 0x0d, 0x50, 0xa6, 0x0a, 0xb3, 0x88, 0xfe, 0xc7, 0x0e,
    ];

    #[test]
    fn keeps_the_colour_under_a_low_alpha() {
        let expected: Vec<u8> = [0, 1, 2, 255].iter().flat_map(|&alpha| [200, 100, 50, alpha]).collect();
        assert_eq!(super::decode(PROBE).unwrap(), expected);
    }
}
