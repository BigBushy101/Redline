//! In-process image conversion with the `image` crate.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use anyhow::{Context, Result, bail};
use image::codecs::avif::AvifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageEncoder, ImageFormat, ImageReader};

use super::convert::ConvertOptions;

pub fn convert(input: &Path, output: &Path, target: &str, o: &ConvertOptions) -> Result<()> {
    let img = ImageReader::open(input)
        .with_context(|| format!("can't open {}", input.display()))?
        .with_guessed_format()?
        .decode()
        .context("couldn't decode image")?;

    let img = o.filters.apply_to_image(img);
    let mut img = resize(img, o.resize_width, o.resize_height);
    if target == "ico" && (img.width() > 256 || img.height() > 256) {
        img = img.resize(256, 256, FilterType::Lanczos3);
    }

    let mut out = BufWriter::new(
        File::create(output).with_context(|| format!("can't write {}", output.display()))?,
    );
    let q = o.image_quality.clamp(1, 100);
    match target {
        "jpg" => {
            JpegEncoder::new_with_quality(&mut out, q).encode_image(&img.to_rgb8())?;
        }
        "png" => {
            img.write_with_encoder(PngEncoder::new_with_quality(
                &mut out,
                CompressionType::Best,
                PngFilter::Adaptive,
            ))?;
        }
        "webp" => {
            WebPEncoder::new_lossless(&mut out).encode(
                img.to_rgba8().as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
        "avif" => {
            AvifEncoder::new_with_speed_quality(&mut out, 6, q).write_image(
                img.to_rgba8().as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
        "bmp" | "gif" | "tiff" | "ico" | "tga" | "qoi" => {
            let fmt = ImageFormat::from_extension(target).expect("known extension");
            // GIF and ICO want RGBA; BMP/TIFF handle whatever we give them.
            let img = DynamicImage::ImageRgba8(img.to_rgba8());
            img.write_to(&mut out, fmt)?;
        }
        other => bail!("unsupported image format: {other}"),
    }
    Ok(())
}

/// Fits the image inside `max_w × max_h` (0 = unconstrained), keeping aspect ratio.
fn resize(img: DynamicImage, max_w: u32, max_h: u32) -> DynamicImage {
    if max_w == 0 && max_h == 0 {
        return img;
    }
    let w = if max_w == 0 { u32::MAX } else { max_w };
    let h = if max_h == 0 { u32::MAX } else { max_h };
    if img.width() <= w && img.height() <= h {
        return img;
    }
    img.resize(w, h, FilterType::Lanczos3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn converts_png_to_each_target() {
        let dir = std::env::temp_dir().join(format!("redline-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.png");
        RgbaImage::from_pixel(64, 32, Rgba([200, 30, 50, 255]))
            .save(&src)
            .unwrap();

        let o = ConvertOptions {
            resize_width: 32,
            ..Default::default()
        };
        for t in ["jpg", "png", "webp", "bmp", "gif", "tiff", "ico", "avif"] {
            let out = dir.join(format!("out.{t}"));
            convert(&src, &out, t, &o).unwrap_or_else(|e| panic!("{t}: {e:#}"));
            assert!(std::fs::metadata(&out).unwrap().len() > 0, "{t} empty");
            if t != "avif" {
                let back = image::open(&out).unwrap();
                assert_eq!((back.width(), back.height()), (32, 16), "{t} size");
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
