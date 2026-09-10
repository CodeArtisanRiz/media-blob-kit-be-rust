pub mod image_processor;

pub fn sanitize_bucket_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
}

pub fn build_s3_url(s3_key: &str) -> String {
    let config = crate::config::get_config();
    let key = s3_key.trim_start_matches('/');
    if let Some(endpoint) = &config.s3_endpoint {
        let endpoint = endpoint.trim_end_matches('/');
        format!("{}/{}/{}", endpoint, config.s3_bucket_name, key)
    } else {
        format!("https://{}.s3.{}.amazonaws.com/{}", config.s3_bucket_name, config.aws_region, key)
    }
}

pub fn extract_s3_key(url_or_key: &str) -> String {
    let config = crate::config::get_config();
    crate::services::s3::S3Service::extract_s3_key(url_or_key, &config.s3_bucket_name)
}

pub fn format_variants_json(variants_json: &serde_json::Value) -> serde_json::Value {
    if let Some(obj) = variants_json.as_object() {
        let mut formatted = serde_json::Map::new();
        for (k, v) in obj {
            if let Some(val_str) = v.as_str() {
                let key = extract_s3_key(val_str);
                formatted.insert(k.clone(), serde_json::Value::String(build_s3_url(&key)));
            } else {
                formatted.insert(k.clone(), v.clone());
            }
        }
        serde_json::Value::Object(formatted)
    } else {
        serde_json::json!({})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_bucket_name() {
        assert_eq!(sanitize_bucket_name("My Project_Name 123!"), "my-project-name-123-");
        assert_eq!(sanitize_bucket_name("clean-slug"), "clean-slug");
    }

    #[test]
    fn test_extract_s3_key() {
        let bucket = "my-bucket";
        assert_eq!(S3Service::extract_s3_key("some/s3/key.png", bucket), "some/s3/key.png");
        assert_eq!(
            S3Service::extract_s3_key("https://my-bucket.s3.us-east-1.amazonaws.com/images/thumb.webp", bucket),
            "images/thumb.webp"
        );
        assert_eq!(
            S3Service::extract_s3_key("https://minio.local/my-bucket/images/thumb.webp", bucket),
            "images/thumb.webp"
        );
    }
}
