use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt;
use object_store::{ObjectStore, aws::AmazonS3Builder, signer::Signer as _};
use tracing::debug;

use super::backend::Backend;
use super::path::{EntryKind, StorageEntry, StoragePath, sort_entries};

pub struct S3Backend {
    store: object_store::aws::AmazonS3,
    bucket: String,
    /// Custom endpoint base URL (e.g. "https://s3.us-west-004.backblazeb2.com").
    /// `None` means standard AWS S3.
    endpoint: Option<String>,
    region: String,
    display_name: String,
}

impl S3Backend {
    pub fn bucket_name(&self) -> &str {
        &self.bucket
    }
}

/// Standard environment variable names.
pub const ENV_BUCKET: &str = "AWS_S3_BUCKET";
pub const ENV_ENDPOINT: &str = "AWS_ENDPOINT_URL";
pub const ENV_ACCESS_KEY: &str = "AWS_ACCESS_KEY_ID";
pub const ENV_SECRET_KEY: &str = "AWS_SECRET_ACCESS_KEY";
pub const ENV_REGION: &str = "AWS_DEFAULT_REGION";

impl S3Backend {
    /// Build entirely from environment variables.
    /// Reads `AWS_S3_BUCKET` for the bucket name plus the standard AWS credential vars.
    /// Returns `Err` if `AWS_S3_BUCKET` is unset or the client cannot be built.
    pub fn from_env() -> Result<Self> {
        let bucket =
            std::env::var(ENV_BUCKET).with_context(|| format!("{ENV_BUCKET} is not set"))?;
        let endpoint = std::env::var(ENV_ENDPOINT).ok().filter(|s| !s.is_empty());
        let region = std::env::var(ENV_REGION).unwrap_or_else(|_| "us-east-1".to_owned());
        let store = AmazonS3Builder::from_env()
            .with_bucket_name(&bucket)
            .build()
            .with_context(|| format!("building S3 client for bucket '{bucket}'"))?;
        Ok(Self {
            store,
            bucket: bucket.clone(),
            endpoint,
            region,
            display_name: format!("S3: {bucket}"),
        })
    }

    /// Explicit credentials — used when env vars are not set.
    ///
    /// When `endpoint` is `Some`, path-style requests are used automatically,
    /// which is required by Backblaze B2, MinIO, and most non-AWS S3 providers.
    pub fn with_credentials(
        bucket: &str,
        endpoint: Option<&str>,
        access_key: &str,
        secret_key: &str,
        region: &str,
    ) -> Result<Self> {
        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(bucket)
            .with_access_key_id(access_key)
            .with_secret_access_key(secret_key)
            .with_region(region);
        if let Some(ep) = endpoint {
            builder = builder
                .with_endpoint(ep)
                // Non-AWS providers (B2, MinIO, …) require path-style URLs:
                // https://<endpoint>/<bucket>/<key>  rather than
                // https://<bucket>.<endpoint>/<key>
                .with_virtual_hosted_style_request(false);
        }
        let store = builder.build().context("building S3 client")?;
        Ok(Self {
            store,
            bucket: bucket.to_owned(),
            endpoint: endpoint.map(str::to_owned),
            region: region.to_owned(),
            display_name: format!("S3: {bucket}"),
        })
    }
}

/// Extract the last path segment, stripping trailing slashes.
fn last_segment(key: &str) -> String {
    key.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(key)
        .to_owned()
}

#[async_trait]
impl Backend for S3Backend {
    async fn list(&self, path: &StoragePath) -> Result<Vec<StorageEntry>> {
        let StoragePath::S3 { bucket, prefix } = path else {
            bail!("S3Backend cannot handle {path:?}");
        };
        if bucket != &self.bucket {
            bail!("S3Backend is for bucket '{}', not '{bucket}'", self.bucket);
        }

        debug!("S3 list s3://{bucket}/{prefix}");

        let os_prefix = if prefix.is_empty() {
            None
        } else {
            Some(object_store::path::Path::from(prefix.as_str()))
        };
        let os_prefix_ref = os_prefix.as_ref();

        let result = self
            .store
            .list_with_delimiter(os_prefix_ref)
            .await
            .with_context(|| format!("listing s3://{bucket}/{prefix}"))?;

        let mut entries: Vec<StorageEntry> = Vec::new();

        // Common prefixes → virtual directories
        for cp in result.common_prefixes {
            let key = cp.to_string(); // e.g. "foo/bar/"
            let name = last_segment(&key);
            entries.push(StorageEntry {
                name,
                path: StoragePath::s3(bucket, &key),
                kind: EntryKind::Directory,
                size: None,
                last_modified: None,
            });
        }

        // Objects → files
        for obj in result.objects {
            let key = obj.location.to_string(); // e.g. "foo/bar/file.txt"
            let name = last_segment(&key);
            if name.is_empty() {
                continue; // skip "directory placeholder" objects (key ends with "/")
            }
            entries.push(StorageEntry {
                name,
                path: StoragePath::s3(bucket, &key),
                kind: EntryKind::File,
                size: Some(obj.size as u64),
                last_modified: Some(obj.last_modified),
            });
        }

        sort_entries(&mut entries);

        Ok(entries)
    }

    async fn get(&self, path: &StoragePath) -> Result<Bytes> {
        let StoragePath::S3 { bucket, prefix } = path else {
            bail!("S3Backend cannot handle {path:?}");
        };
        debug!("S3 get s3://{bucket}/{prefix}");
        let os_path = object_store::path::Path::from(prefix.as_str());
        let result = self
            .store
            .get(&os_path)
            .await
            .with_context(|| format!("downloading s3://{bucket}/{prefix}"))?;
        Ok(result.bytes().await?)
    }

    async fn put(&self, path: &StoragePath, data: Bytes) -> Result<()> {
        let StoragePath::S3 { bucket, prefix } = path else {
            bail!("S3Backend cannot handle {path:?}");
        };
        debug!("S3 put s3://{bucket}/{prefix} ({} bytes)", data.len());
        let os_path = object_store::path::Path::from(prefix.as_str());
        self.store
            .put(&os_path, data.into())
            .await
            .with_context(|| format!("uploading to s3://{bucket}/{prefix}"))?;
        Ok(())
    }

    async fn delete(&self, path: &StoragePath) -> Result<()> {
        let StoragePath::S3 { bucket, prefix } = path else {
            bail!("S3Backend cannot handle {path:?}");
        };
        if prefix.ends_with('/') || prefix.is_empty() {
            // Virtual directory: delete every object with this prefix.
            let os_prefix = object_store::path::Path::from(prefix.trim_end_matches('/'));
            debug!("S3 delete-prefix s3://{bucket}/{prefix}");
            let locations: Vec<_> = self
                .store
                .list(Some(&os_prefix))
                .map_ok(|m| m.location)
                .try_collect()
                .await?;
            for loc in locations {
                self.store.delete(&loc).await
                    .with_context(|| format!("deleting s3://{bucket}/{loc}"))?;
            }
        } else {
            debug!("S3 delete s3://{bucket}/{prefix}");
            let os_path = object_store::path::Path::from(prefix.as_str());
            self.store.delete(&os_path).await
                .with_context(|| format!("deleting s3://{bucket}/{prefix}"))?;
        }
        Ok(())
    }

    fn public_url(&self, path: &StoragePath) -> Option<String> {
        let StoragePath::S3 { bucket, prefix } = path else { return None; };
        let key = prefix.trim_end_matches('/');
        Some(match &self.endpoint {
            // Custom endpoint (Backblaze, MinIO, …): path-style URL.
            Some(ep) => format!("{}/{}/{}", ep.trim_end_matches('/'), bucket, key),
            // AWS: virtual-hosted style with region.
            None => format!(
                "https://{}.s3.{}.amazonaws.com/{}",
                bucket, self.region, key
            ),
        })
    }

    async fn presign_url(&self, path: &StoragePath, expires: Duration) -> Result<String> {
        let StoragePath::S3 { prefix, .. } = path else {
            bail!("not an S3 path");
        };
        let os_path = object_store::path::Path::from(prefix.as_str());
        let url = self.store
            .signed_url(http::Method::GET, &os_path, expires)
            .await
            .context("generating presigned URL")?;
        Ok(url.to_string())
    }

    fn name(&self) -> &str {
        &self.display_name
    }
}
