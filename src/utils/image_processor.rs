use image::{ImageFormat, ImageReader, Limits};
use std::io::Cursor;
use crate::models::settings::VariantConfig;
use crate::error::AppError;

pub fn process_image(data: &[u8], config: &VariantConfig) -> Result<(Vec<u8>, String), AppError> {
    // 1. Load image with strict memory and dimension limits to prevent OOM attacks
    let mut reader = ImageReader::new(Cursor::new(data));
    reader = reader.with_guessed_format()
        .map_err(|e| AppError::BadRequest(format!("Unsupported image format: {}", e)))?;

    let mut limits = Limits::default();
    limits.max_image_width = Some(10_000);  // Cap at 10k pixels width
    limits.max_image_height = Some(10_000); // Cap at 10k pixels height
    limits.max_alloc = Some(128 * 1024 * 1024); // Cap decoding buffer allocation to 128MB RAM
    reader.limits(limits);

    let mut img = reader.decode()
        .map_err(|e| AppError::InternalServerError(format!("Failed to decode image safely: {}", e)))?;

    // 2. Optimized Filter & Resizing
    // Use Triangle/Nearest for very small thumbnails, Lanczos3 for high quality previews
    let is_thumb = config.width.map(|w| w <= 200).unwrap_or(false) || config.height.map(|h| h <= 200).unwrap_or(false);
    let filter = if is_thumb {
        image::imageops::FilterType::Triangle
    } else {
        image::imageops::FilterType::Lanczos3
    };

    let fit = config.fit.as_deref().unwrap_or("contain");

    if let (Some(w), Some(h)) = (config.width, config.height) {
        match fit {
            "cover" | "center-crop" => {
                img = img.resize_to_fill(w, h, filter);
            },
            "fill" | "stretch" | "exact" => {
                img = img.resize_exact(w, h, filter);
            },
            _ => {
                img = img.resize(w, h, filter);
            }
        }
    } else if let Some(w) = config.width {
        img = img.resize(w, u32::MAX, filter);
    } else if let Some(h) = config.height {
        img = img.resize(u32::MAX, h, filter);
    } else if let (Some(w), Some(h)) = (config.max_width, config.max_height) {
        img = img.resize(w, h, filter);
    }

    // 3. Determine Output Format
    let format_str = config.format.as_deref().unwrap_or("original");
    let (output_format, mime_type) = match format_str {
        "avif" => (ImageFormat::Avif, "image/avif"),
        "webp" => (ImageFormat::WebP, "image/webp"),
        "png" => (ImageFormat::Png, "image/png"),
        "jpg" | "jpeg" => (ImageFormat::Jpeg, "image/jpeg"),
        "original" => {
            let fmt = image::guess_format(data)
                .map_err(|e| AppError::InternalServerError(format!("Failed to guess format: {}", e)))?;
            let mime = match fmt {
                ImageFormat::Avif => "image/avif",
                ImageFormat::WebP => "image/webp",
                ImageFormat::Png => "image/png",
                ImageFormat::Jpeg => "image/jpeg",
                _ => "application/octet-stream",
            };
            (fmt, mime)
        },
        _ => (ImageFormat::Jpeg, "image/jpeg"),
    };

    // 4. Encode with Quality
    let mut buffer = Cursor::new(Vec::new());
    let quality = config.quality.unwrap_or(80);

    match output_format {
        ImageFormat::Jpeg => {
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buffer, quality);
            img.write_with_encoder(encoder)
                .map_err(|e| AppError::InternalServerError(format!("Failed to encode JPEG: {}", e)))?;
        }
        _ => {
            img.write_to(&mut buffer, output_format)
                .map_err(|e| AppError::InternalServerError(format!("Failed to encode image: {}", e)))?;
        }
    }

    Ok((buffer.into_inner(), mime_type.to_string()))
}
