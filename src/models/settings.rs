use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectSettings {
    pub variants: Option<HashMap<String, VariantConfig>>,
    /// If true (default), keep the original uploaded image in S3 permanently.
    /// If false, delete the original from S3 after all variants are successfully processed.
    #[serde(default = "default_keep_original")]
    pub keep_original: bool,
    /// Maximum upload size in bytes. None means use server default.
    #[serde(default)]
    pub max_upload_bytes: Option<i64>,
}

fn default_keep_original() -> bool {
    true
}

impl ProjectSettings {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(max) = self.max_upload_bytes {
            if max <= 0 {
                return Err("max_upload_bytes must be positive".to_string());
            }
        }
        if let Some(ref variants) = self.variants {
            for (name, config) in variants {
                config.validate().map_err(|e| format!("variant '{}': {}", name, e))?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantConfig {
    pub format: Option<String>,
    pub quality: Option<u8>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub fit: Option<String>, // cover, contain, inside, fill
}

const VALID_FORMATS: &[&str] = &["webp", "jpg", "jpeg", "png", "avif", "original"];
const VALID_FITS: &[&str] = &["cover", "contain", "fill", "inside"];

impl VariantConfig {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(q) = self.quality {
            if q < 1 || q > 100 {
                return Err("quality must be between 1 and 100".to_string());
            }
        }
        if let Some(w) = self.width {
            if w < 1 || w > 10000 {
                return Err("width must be between 1 and 10000".to_string());
            }
        }
        if let Some(h) = self.height {
            if h < 1 || h > 10000 {
                return Err("height must be between 1 and 10000".to_string());
            }
        }
        if let Some(w) = self.max_width {
            if w < 1 || w > 10000 {
                return Err("max_width must be between 1 and 10000".to_string());
            }
        }
        if let Some(h) = self.max_height {
            if h < 1 || h > 10000 {
                return Err("max_height must be between 1 and 10000".to_string());
            }
        }
        if let Some(ref fmt) = self.format {
            if !VALID_FORMATS.contains(&fmt.as_str()) {
                return Err(format!("format must be one of: {}", VALID_FORMATS.join(", ")));
            }
        }
        if let Some(ref fit) = self.fit {
            if !VALID_FITS.contains(&fit.as_str()) {
                return Err(format!("fit must be one of: {}", VALID_FITS.join(", ")));
            }
        }
        Ok(())
    }
}
