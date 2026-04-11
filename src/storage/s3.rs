use std::time::Duration;

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use tracing::debug;

use super::backend::Backend;
use super::path::{EntryKind, StorageEntry, StoragePath, sort_entries};

/// Environment variable names used by this application.
pub const ENV_BUCKET: &str = "S3_BUCKET";
pub const ENV_ENDPOINT: &str = "S3_ENDPOINT_URL";
pub const ENV_ACCESS_KEY: &str = "S3_ACCESS_KEY_ID";
pub const ENV_SECRET_KEY: &str = "S3_SECRET_ACCESS_KEY";
pub const ENV_REGION: &str = "S3_REGION";

/// Explicit credentials for constructing an [`S3Backend`].
pub struct S3Config<'a> {
    pub bucket: &'a str,
    /// Custom endpoint URL (e.g. `"https://s3.us-west-004.backblazeb2.com"`).
    /// `None` uses standard AWS S3.
    pub endpoint: Option<&'a str>,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub region: &'a str,
}

// ─────────────────────────────────────────────────────────────────────────────
// S3Backend struct — fields differ per platform
// ─────────────────────────────────────────────────────────────────────────────

pub struct S3Backend {
    #[cfg(not(target_arch = "wasm32"))]
    store: object_store::aws::AmazonS3,
    #[cfg(target_arch = "wasm32")]
    client: reqwest::Client,
    bucket: String,
    endpoint: Option<String>,
    #[cfg(target_arch = "wasm32")]
    access_key: String,
    #[cfg(target_arch = "wasm32")]
    secret_key: String,
    region: String,
    display_name: String,
}

impl S3Backend {
    pub fn bucket_name(&self) -> &str {
        &self.bucket
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Constructors — native (object_store)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
impl S3Backend {
    /// Build from the application's `S3_*` environment variables.
    pub fn from_env() -> Result<Self> {
        let bucket =
            std::env::var(ENV_BUCKET).with_context(|| format!("{ENV_BUCKET} is not set"))?;
        let access_key = std::env::var(ENV_ACCESS_KEY)
            .with_context(|| format!("{ENV_ACCESS_KEY} is not set"))?;
        let secret_key = std::env::var(ENV_SECRET_KEY)
            .with_context(|| format!("{ENV_SECRET_KEY} is not set"))?;
        let endpoint = std::env::var(ENV_ENDPOINT).ok().filter(|s| !s.is_empty());
        let region = std::env::var(ENV_REGION).unwrap_or_else(|_| "us-east-1".to_owned());
        Self::with_credentials(S3Config {
            bucket: &bucket,
            endpoint: endpoint.as_deref(),
            access_key: &access_key,
            secret_key: &secret_key,
            region: &region,
        })
    }

    /// Explicit credentials.
    pub fn with_credentials(config: S3Config<'_>) -> Result<Self> {
        use object_store::aws::AmazonS3Builder;
        let S3Config { bucket, endpoint, access_key, secret_key, region } = config;
        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(bucket)
            .with_access_key_id(access_key)
            .with_secret_access_key(secret_key)
            .with_region(region);
        if let Some(ep) = endpoint {
            builder = builder
                .with_endpoint(ep)
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

// ─────────────────────────────────────────────────────────────────────────────
// Constructors — WASM (reqwest + manual Sig V4)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
impl S3Backend {
    pub fn from_env() -> Result<Self> {
        let bucket =
            std::env::var(ENV_BUCKET).with_context(|| format!("{ENV_BUCKET} is not set"))?;
        let access_key = std::env::var(ENV_ACCESS_KEY)
            .with_context(|| format!("{ENV_ACCESS_KEY} is not set"))?;
        let secret_key = std::env::var(ENV_SECRET_KEY)
            .with_context(|| format!("{ENV_SECRET_KEY} is not set"))?;
        let endpoint = std::env::var(ENV_ENDPOINT).ok().filter(|s| !s.is_empty());
        let region = std::env::var(ENV_REGION).unwrap_or_else(|_| "us-east-1".to_owned());
        Self::with_credentials(S3Config {
            bucket: &bucket,
            endpoint: endpoint.as_deref(),
            access_key: &access_key,
            secret_key: &secret_key,
            region: &region,
        })
    }

    pub fn with_credentials(config: S3Config<'_>) -> Result<Self> {
        let S3Config { bucket, endpoint, access_key, secret_key, region } = config;
        Ok(Self {
            client: reqwest::Client::new(),
            bucket: bucket.to_owned(),
            endpoint: endpoint.map(str::to_owned),
            access_key: access_key.to_owned(),
            secret_key: secret_key.to_owned(),
            region: region.to_owned(),
            display_name: format!("S3: {bucket}"),
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared helper
// ─────────────────────────────────────────────────────────────────────────────

/// Extract the last path segment, stripping trailing slashes.
fn last_segment(key: &str) -> String {
    key.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(key)
        .to_owned()
}

// ─────────────────────────────────────────────────────────────────────────────
// Backend impl — native (object_store)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl Backend for S3Backend {
    async fn list(&self, path: &StoragePath) -> Result<Vec<StorageEntry>> {
        use object_store::ObjectStore;
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
        let result = self
            .store
            .list_with_delimiter(os_prefix.as_ref())
            .await
            .with_context(|| format!("listing s3://{bucket}/{prefix}"))?;

        let mut entries: Vec<StorageEntry> = Vec::new();
        for cp in result.common_prefixes {
            let key = cp.to_string();
            let name = last_segment(&key);
            entries.push(StorageEntry {
                name,
                path: StoragePath::s3(bucket, &key),
                kind: EntryKind::Directory,
                size: None,
                last_modified: None,
            });
        }
        for obj in result.objects {
            let key = obj.location.to_string();
            let name = last_segment(&key);
            if name.is_empty() {
                continue;
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
        use object_store::ObjectStore;
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
        use object_store::ObjectStore;
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
        use futures::TryStreamExt;
        use object_store::ObjectStore;
        let StoragePath::S3 { bucket, prefix } = path else {
            bail!("S3Backend cannot handle {path:?}");
        };
        if prefix.ends_with('/') || prefix.is_empty() {
            let os_prefix =
                object_store::path::Path::from(prefix.trim_end_matches('/'));
            debug!("S3 delete-prefix s3://{bucket}/{prefix}");
            let locations: Vec<_> = self
                .store
                .list(Some(&os_prefix))
                .map_ok(|m| m.location)
                .try_collect()
                .await?;
            for loc in locations {
                self.store
                    .delete(&loc)
                    .await
                    .with_context(|| format!("deleting s3://{bucket}/{loc}"))?;
            }
        } else {
            debug!("S3 delete s3://{bucket}/{prefix}");
            let os_path = object_store::path::Path::from(prefix.as_str());
            self.store
                .delete(&os_path)
                .await
                .with_context(|| format!("deleting s3://{bucket}/{prefix}"))?;
        }
        Ok(())
    }

    fn public_url(&self, path: &StoragePath) -> Option<String> {
        let StoragePath::S3 { bucket, prefix } = path else {
            return None;
        };
        let key = prefix.trim_end_matches('/');
        Some(match &self.endpoint {
            Some(ep) => format!("{}/{}/{}", ep.trim_end_matches('/'), bucket, key),
            None => format!(
                "https://{}.s3.{}.amazonaws.com/{}",
                bucket, self.region, key
            ),
        })
    }

    async fn presign_url(&self, path: &StoragePath, expires: Duration) -> Result<String> {
        use object_store::signer::Signer as _;
        let StoragePath::S3 { prefix, .. } = path else {
            bail!("not an S3 path");
        };
        let os_path = object_store::path::Path::from(prefix.as_str());
        let url = self
            .store
            .signed_url(http::Method::GET, &os_path, expires)
            .await
            .context("generating presigned URL")?;
        Ok(url.to_string())
    }

    fn name(&self) -> &str {
        &self.display_name
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Backend impl — WASM (reqwest + manual AWS Signature V4)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod wasm_helpers {
    use anyhow::{Result, bail};
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};

    type HmacSha256 = Hmac<Sha256>;

    pub fn hex_sha256(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    pub fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }

    /// `(key, size_bytes, last_modified_rfc3339)`
    type S3Object = (String, u64, String);

    /// Parse the ListObjectsV2 XML response.
    /// Returns (common_prefixes, objects)
    pub fn parse_list_objects(xml: &str) -> Result<(Vec<String>, Vec<S3Object>)> {
        use quick_xml::Reader;
        use quick_xml::events::Event;

        let mut prefixes: Vec<String> = Vec::new();
        let mut objects: Vec<(String, u64, String)> = Vec::new();

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut in_common_prefix = false;
        let mut in_contents = false;
        let mut current_key = String::new();
        let mut current_size: u64 = 0;
        let mut current_last_modified = String::new();
        let mut buf = Vec::new();
        let mut tag_stack: Vec<String> = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let name =
                        String::from_utf8_lossy(e.name().as_ref()).to_string();
                    match name.as_str() {
                        "CommonPrefixes" => in_common_prefix = true,
                        "Contents" => in_contents = true,
                        _ => {}
                    }
                    tag_stack.push(name);
                }
                Ok(Event::End(_)) => {
                    let closed = tag_stack.pop().unwrap_or_default();
                    match closed.as_str() {
                        "CommonPrefixes" => in_common_prefix = false,
                        "Contents" => {
                            if in_contents && !current_key.is_empty() {
                                objects.push((
                                    current_key.clone(),
                                    current_size,
                                    current_last_modified.clone(),
                                ));
                            }
                            current_key.clear();
                            current_size = 0;
                            current_last_modified.clear();
                            in_contents = false;
                        }
                        _ => {}
                    }
                }
                Ok(Event::Text(e)) => {
                    let text = e.unescape().unwrap_or_default().to_string();
                    let tag = tag_stack.last().map(|s| s.as_str()).unwrap_or("");
                    if in_common_prefix && tag == "Prefix" {
                        prefixes.push(text);
                    } else if in_contents {
                        match tag {
                            "Key" => current_key = text,
                            "Size" => current_size = text.parse().unwrap_or(0),
                            "LastModified" => current_last_modified = text,
                            _ => {}
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => bail!("XML parse error: {e}"),
                _ => {}
            }
            buf.clear();
        }

        Ok((prefixes, objects))
    }

    pub fn extract_next_continuation_token(xml: &str) -> Option<String> {
        use quick_xml::Reader;
        use quick_xml::events::Event;
        let mut reader = Reader::from_str(xml);
        let mut buf = Vec::new();
        let mut in_token = false;
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    if e.name().as_ref() == b"NextContinuationToken" {
                        in_token = true;
                    }
                }
                Ok(Event::Text(e)) if in_token => {
                    return Some(e.unescape().unwrap_or_default().to_string());
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }
        None
    }
}

/// On WASM, a network-level send failure is almost always a CORS block.
/// The browser hides the real cause and just reports "Failed to fetch" or similar.
/// Tag the error with a recognisable prefix so the UI can show targeted help.
#[cfg(target_arch = "wasm32")]
fn cors_hint(e: reqwest::Error) -> anyhow::Error {
    anyhow::anyhow!("CORS_ERROR: {e}")
}

#[cfg(target_arch = "wasm32")]
impl S3Backend {
    /// Build the host string for Authorization header.
    fn sig_host(&self) -> String {
        match &self.endpoint {
            Some(ep) => ep
                .trim_end_matches('/')
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_owned(),
            None => format!("{}.s3.{}.amazonaws.com", self.bucket, self.region),
        }
    }

    /// URL + request path for an object key.
    fn object_url_path(&self, key: &str) -> (String, String) {
        match &self.endpoint {
            Some(ep) => (
                format!("{}/{}/{}", ep.trim_end_matches('/'), self.bucket, key),
                format!("/{}/{}", self.bucket, key),
            ),
            None => (
                format!(
                    "https://{}.s3.{}.amazonaws.com/{}",
                    self.bucket, self.region, key
                ),
                format!("/{}", key),
            ),
        }
    }

    /// URL + request path for bucket-level operations (list, etc.)
    fn bucket_url_path(&self) -> (String, String) {
        match &self.endpoint {
            Some(ep) => (
                format!("{}/{}/", ep.trim_end_matches('/'), self.bucket),
                format!("/{}/", self.bucket),
            ),
            None => (
                format!("https://{}.s3.{}.amazonaws.com/", self.bucket, self.region),
                "/".to_owned(),
            ),
        }
    }

    /// Sign with AWS Signature Version 4 and return headers to add.
    fn sign_v4(
        &self,
        method: &str,
        req_path: &str,
        query: &str,
        body_sha256: &str,
        now: &chrono::DateTime<chrono::Utc>,
    ) -> Vec<(String, String)> {
        use wasm_helpers::{hmac_sha256, hex_sha256};

        let date_stamp = now.format("%Y%m%d").to_string();
        let datetime_stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let host = self.sig_host();

        let canonical_headers = format!(
            "host:{host}\nx-amz-content-sha256:{body_sha256}\nx-amz-date:{datetime_stamp}\n"
        );
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";
        let canonical_request = format!(
            "{method}\n{req_path}\n{query}\n{canonical_headers}\n{signed_headers}\n{body_sha256}"
        );
        let credential_scope = format!("{date_stamp}/{}/s3/aws4_request", self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{datetime_stamp}\n{credential_scope}\n{}",
            hex_sha256(canonical_request.as_bytes())
        );

        let k_date = hmac_sha256(
            format!("AWS4{}", self.secret_key).as_bytes(),
            date_stamp.as_bytes(),
        );
        let k_region = hmac_sha256(&k_date, self.region.as_bytes());
        let k_service = hmac_sha256(&k_region, b"s3");
        let signing_key = hmac_sha256(&k_service, b"aws4_request");
        let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

        let auth = format!(
            "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, \
             SignedHeaders={signed_headers}, Signature={signature}",
            self.access_key,
        );

        vec![
            ("Host".to_owned(), host),
            ("x-amz-date".to_owned(), datetime_stamp),
            ("x-amz-content-sha256".to_owned(), body_sha256.to_owned()),
            ("Authorization".to_owned(), auth),
        ]
    }

    async fn do_get(&self, url: &str, req_path: &str, query: &str) -> Result<reqwest::Response> {
        const EMPTY_SHA: &str =
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let now = chrono::Utc::now();
        let headers = self.sign_v4("GET", req_path, query, EMPTY_SHA, &now);
        let full_url = if query.is_empty() {
            url.to_owned()
        } else {
            format!("{url}?{query}")
        };
        let mut req = self.client.get(&full_url);
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.send().await.map_err(cors_hint)?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("S3 GET {status}: {body}");
        }
        Ok(resp)
    }

    async fn do_put(&self, url: &str, req_path: &str, data: Bytes) -> Result<()> {
        use wasm_helpers::hex_sha256;
        let body_sha256 = hex_sha256(&data);
        let now = chrono::Utc::now();
        let headers = self.sign_v4("PUT", req_path, "", &body_sha256, &now);
        let mut req = self.client.put(url).body(data);
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.send().await.map_err(cors_hint)?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("S3 PUT {status}: {body}");
        }
        Ok(())
    }

    async fn do_delete(&self, url: &str, req_path: &str) -> Result<()> {
        const EMPTY_SHA: &str =
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let now = chrono::Utc::now();
        let headers = self.sign_v4("DELETE", req_path, "", EMPTY_SHA, &now);
        let mut req = self.client.delete(url);
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.send().await.map_err(cors_hint)?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("S3 DELETE {status}: {body}");
        }
        Ok(())
    }

    /// List all keys under a prefix without a delimiter (for recursive delete).
    async fn list_all_keys(&self, prefix: &str) -> Result<Vec<String>> {
        let (url, req_path) = self.bucket_url_path();
        let mut all_keys = Vec::new();
        let mut continuation: Option<String> = None;

        loop {
            let mut query =
                format!("list-type=2&prefix={}", urlencoding::encode(prefix));
            if let Some(ref token) = continuation {
                query.push_str(&format!(
                    "&continuation-token={}",
                    urlencoding::encode(token)
                ));
            }
            let xml = self.do_get(&url, &req_path, &query).await?.text().await?;
            let (_, objects) = wasm_helpers::parse_list_objects(&xml)?;
            for (key, _, _) in objects {
                all_keys.push(key);
            }
            continuation = wasm_helpers::extract_next_continuation_token(&xml);
            if continuation.is_none() {
                break;
            }
        }
        Ok(all_keys)
    }
}

#[cfg(target_arch = "wasm32")]
#[async_trait::async_trait(?Send)]
impl Backend for S3Backend {
    async fn list(&self, path: &StoragePath) -> Result<Vec<StorageEntry>> {
        let StoragePath::S3 { bucket, prefix } = path else {
            bail!("S3Backend cannot handle {path:?}");
        };
        if bucket != &self.bucket {
            bail!("S3Backend is for bucket '{}', not '{bucket}'", self.bucket);
        }
        debug!("S3 list s3://{bucket}/{prefix}");

        let (url, req_path) = self.bucket_url_path();
        let mut query = "list-type=2".to_owned();
        if !prefix.is_empty() {
            query.push_str(&format!("&prefix={}", urlencoding::encode(prefix)));
        }
        query.push_str("&delimiter=%2F"); // delimiter=/

        let xml = self.do_get(&url, &req_path, &query).await?.text().await?;
        let (common_prefixes, objects) = wasm_helpers::parse_list_objects(&xml)?;

        let mut entries: Vec<StorageEntry> = Vec::new();
        for cp in common_prefixes {
            let name = last_segment(&cp);
            entries.push(StorageEntry {
                name,
                path: StoragePath::s3(bucket, &cp),
                kind: EntryKind::Directory,
                size: None,
                last_modified: None,
            });
        }
        for (key, size, last_mod) in objects {
            let name = last_segment(&key);
            if name.is_empty() {
                continue;
            }
            let last_modified = chrono::DateTime::parse_from_rfc3339(&last_mod)
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc));
            entries.push(StorageEntry {
                name,
                path: StoragePath::s3(bucket, &key),
                kind: EntryKind::File,
                size: Some(size),
                last_modified,
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
        let (url, req_path) = self.object_url_path(prefix);
        let resp = self.do_get(&url, &req_path, "").await?;
        Ok(resp.bytes().await.context("reading response body")?)
    }

    async fn put(&self, path: &StoragePath, data: Bytes) -> Result<()> {
        let StoragePath::S3 { bucket, prefix } = path else {
            bail!("S3Backend cannot handle {path:?}");
        };
        debug!("S3 put s3://{bucket}/{prefix} ({} bytes)", data.len());
        let (url, req_path) = self.object_url_path(prefix);
        self.do_put(&url, &req_path, data).await
    }

    async fn delete(&self, path: &StoragePath) -> Result<()> {
        let StoragePath::S3 { bucket, prefix } = path else {
            bail!("S3Backend cannot handle {path:?}");
        };
        if prefix.ends_with('/') || prefix.is_empty() {
            debug!("S3 delete-prefix s3://{bucket}/{prefix}");
            let keys = self.list_all_keys(prefix).await?;
            for key in keys {
                let (url, req_path) = self.object_url_path(&key);
                self.do_delete(&url, &req_path).await?;
            }
        } else {
            debug!("S3 delete s3://{bucket}/{prefix}");
            let (url, req_path) = self.object_url_path(prefix);
            self.do_delete(&url, &req_path).await?;
        }
        Ok(())
    }

    fn public_url(&self, path: &StoragePath) -> Option<String> {
        let StoragePath::S3 { bucket, prefix } = path else {
            return None;
        };
        let key = prefix.trim_end_matches('/');
        Some(match &self.endpoint {
            Some(ep) => format!("{}/{}/{}", ep.trim_end_matches('/'), bucket, key),
            None => format!(
                "https://{}.s3.{}.amazonaws.com/{}",
                bucket, self.region, key
            ),
        })
    }

    async fn presign_url(&self, path: &StoragePath, _expires: Duration) -> Result<String> {
        // Pre-signed URL generation via query-string signing is complex;
        // fall back to public URL for the browser build.
        self.public_url(path).context("cannot generate URL for this path")
    }

    fn name(&self) -> &str {
        &self.display_name
    }
}
