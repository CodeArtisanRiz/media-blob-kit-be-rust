use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::ByteStream;
use crate::config::get_config;
use crate::error::AppError;

#[derive(Clone)]
pub struct S3Service {
    client: Client,
    pub bucket_name: String,
}

impl S3Service {
    pub async fn new() -> Self {
        let config = get_config();
        
        let credentials = aws_sdk_s3::config::Credentials::new(
            config.aws_access_key_id.clone(),
            config.aws_secret_access_key.clone(),
            None,
            None,
            "manual_config",
        );

        let region = aws_sdk_s3::config::Region::new(config.aws_region.clone());
        
        let mut s3_config_builder = aws_sdk_s3::config::Builder::new()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(region)
            .credentials_provider(credentials);
        
        if let Some(endpoint) = &config.s3_endpoint {
            s3_config_builder = s3_config_builder
                .endpoint_url(endpoint)
                .force_path_style(true);
        }

        let client = Client::from_conf(s3_config_builder.build());

        Self {
            client,
            bucket_name: config.s3_bucket_name.clone(),
        }
    }

    pub async fn put_object(
        &self,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
    ) -> Result<(), AppError> {
        self.client
            .put_object()
            .bucket(&self.bucket_name)
            .key(key)
            .body(ByteStream::from(data))
            .content_type(content_type)
            .acl(aws_sdk_s3::types::ObjectCannedAcl::PublicRead)
            .send()
            .await
            .map_err(|e| {
                eprintln!("S3 Upload Error: {:?}", e);
                AppError::InternalServerError(format!("Failed to upload file to S3: {}", e))
            })?;

        Ok(())
    }

    pub async fn get_object(&self, key: &str) -> Result<Vec<u8>, AppError> {
        let resp = self.client
            .get_object()
            .bucket(&self.bucket_name)
            .key(key)
            .send()
            .await
            .map_err(|e| {
                eprintln!("S3 Download Error: {:?}", e);
                AppError::InternalServerError(format!("Failed to download file from S3: {}", e))
            })?;

        let data = resp.body.collect().await.map_err(|e| {
             eprintln!("S3 Body Error: {:?}", e);
             AppError::InternalServerError("Failed to read S3 body".to_string())
        })?;

        Ok(data.into_bytes().to_vec())
    }

    pub async fn ensure_bucket_exists(&self) -> Result<(), AppError> {
        let resp = self.client.head_bucket().bucket(&self.bucket_name).send().await;
        
        match resp {
            Ok(_) => {
                // Bucket exists, ensure public policy
                self.set_public_policy().await?;
                Ok(())
            },
            Err(_) => {
                // Bucket doesn't exist or no access, try to create it
                println!("Bucket {} does not exist, attempting to create...", self.bucket_name);
                self.client
                    .create_bucket()
                    .bucket(&self.bucket_name)
                    .send()
                    .await
                    .map_err(|e| {
                        eprintln!("Failed to create bucket: {:?}", e);
                        AppError::InternalServerError(format!("Failed to create S3 bucket: {}", e))
                    })?;
                
                // Set public policy after creation
                self.set_public_policy().await?;
                Ok(())
            }
        }
    }

    async fn set_public_policy(&self) -> Result<(), AppError> {
        let policy = format!(
            r#"{{
                "Version": "2012-10-17",
                "Statement": [
                    {{
                        "Sid": "PublicReadGetObject",
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "s3:GetObject",
                        "Resource": "arn:aws:s3:::{}/*"
                    }}
                ]
            }}"#,
            self.bucket_name
        );

        self.client
            .put_bucket_policy()
            .bucket(&self.bucket_name)
            .policy(policy)
            .send()
            .await
            .map_err(|e| {
                eprintln!("Failed to set bucket policy: {:?}", e);
                // Don't fail the request if policy setting fails, just log it
                // Some S3 providers might not support this or require different permissions
                AppError::InternalServerError(format!("Failed to set bucket policy: {}", e))
            })?;
            
        Ok(())
    }



    pub async fn delete_object(&self, key: &str) -> Result<(), AppError> {
        self.client
            .delete_object()
            .bucket(&self.bucket_name)
            .key(key)
            .send()
            .await
            .map_err(|e| {
                eprintln!("S3 Delete Error: {}", e);
                AppError::InternalServerError("Failed to delete file from S3".to_string())
            })?;

        Ok(())
    }

    pub async fn get_presigned_url(
        &self, 
        key: &str, 
        expires_in: std::time::Duration
    ) -> Result<String, AppError> {
        let presigning_config = aws_sdk_s3::presigning::PresigningConfig::expires_in(expires_in)
            .map_err(|e| {
                eprintln!("Presigning Config Error: {}", e);
                AppError::InternalServerError("Failed to configure presigner".to_string())
            })?;

        let presigned_req = self.client
            .get_object()
            .bucket(&self.bucket_name)
            .key(key)
            .presigned(presigning_config)
            .await
            .map_err(|e| {
                eprintln!("Presigning Error: {}", e);
                AppError::InternalServerError("Failed to generate presigned URL".to_string())
            })?;

        Ok(presigned_req.uri().to_string())
    }

    pub fn get_public_url(&self, key: &str) -> String {
        let config = get_config();
        let key = key.trim_start_matches('/');
        if let Some(endpoint) = &config.s3_endpoint {
            let endpoint = endpoint.trim_end_matches('/');
            format!("{}/{}/{}", endpoint, self.bucket_name, key)
        } else {
            format!("https://{}.s3.{}.amazonaws.com/{}", self.bucket_name, config.aws_region, key)
        }
    }

    pub fn extract_s3_key(url_or_key: &str, bucket: &str) -> String {
        let trimmed = url_or_key.trim();
        if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
            return trimmed.to_string();
        }

        if let Some(idx) = trimmed.find(&format!("/{}/", bucket)) {
            trimmed[idx + bucket.len() + 2..].to_string()
        } else if let Ok(parsed) = url::Url::parse(trimmed) {
            parsed.path().trim_start_matches('/').to_string()
        } else {
            trimmed.to_string()
        }
    }
}
