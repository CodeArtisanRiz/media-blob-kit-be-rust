use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectSettings {
    pub variants: Option<HashMap<String, VariantConfig>>,
    /// If true (default), keep the original uploaded image in S3 permanently.
    /// If false, delete the original from S3 after all variants are successfully processed.
    #[serde(default = "default_keep_original")]
    pub keep_original: bool,
}

fn default_keep_original() -> bool {
    true
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
