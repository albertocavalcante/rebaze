//! Registry HTTP client implementation.
//!
//! This module provides HTTP clients for fetching module information from
//! Bazel Central Registry (BCR) and compatible registries.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use tokio::sync::RwLock;
use tracing::{debug, instrument, warn};

use crate::error::RegistryError;
use crate::label::{ModuleName, Version};
use crate::{ModuleInfo, ModuleMetadata, Result, SourceInfo};

/// Registry interface for fetching module information.
#[async_trait]
pub trait Registry: Send + Sync + std::fmt::Debug {
    /// Get module metadata (versions, yanked info, etc.).
    async fn get_metadata(&self, module: &str) -> Result<ModuleMetadata>;

    /// Get MODULE.bazel content for a specific version.
    async fn get_module(&self, module: &str, version: &str) -> Result<ModuleInfo>;

    /// Get source information for a module version.
    async fn get_source(&self, module: &str, version: &str) -> Result<SourceInfo>;

    /// Check if a module exists in this registry.
    async fn has_module(&self, module: &str) -> Result<bool>;
}

/// Cache entry with metadata and expiration tracking.
#[derive(Debug, Clone)]
struct CacheEntry<T> {
    value: T,
    inserted_at: std::time::Instant,
}

impl<T> CacheEntry<T> {
    fn new(value: T) -> Self {
        Self {
            value,
            inserted_at: std::time::Instant::now(),
        }
    }

    fn is_expired(&self, ttl: Duration) -> bool {
        self.inserted_at.elapsed() > ttl
    }
}

/// In-memory cache for registry responses.
#[derive(Debug)]
struct RegistryCache {
    /// Cached metadata by module name.
    metadata: RwLock<HashMap<String, CacheEntry<ModuleMetadata>>>,
    /// Cached module info by "module@version" key.
    modules: RwLock<HashMap<String, CacheEntry<ModuleInfo>>>,
    /// Cached source info by "module@version" key.
    sources: RwLock<HashMap<String, CacheEntry<SourceInfo>>>,
    /// Cache TTL.
    ttl: Duration,
}

impl RegistryCache {
    fn new(ttl: Duration) -> Self {
        Self {
            metadata: RwLock::new(HashMap::new()),
            modules: RwLock::new(HashMap::new()),
            sources: RwLock::new(HashMap::new()),
            ttl,
        }
    }

    fn module_key(module: &str, version: &str) -> String {
        format!("{module}@{version}")
    }

    async fn get_metadata(&self, module: &str) -> Option<ModuleMetadata> {
        let cache = self.metadata.read().await;
        cache.get(module).and_then(|entry| {
            if entry.is_expired(self.ttl) {
                None
            } else {
                Some(entry.value.clone())
            }
        })
    }

    async fn set_metadata(&self, module: &str, metadata: ModuleMetadata) {
        let mut cache = self.metadata.write().await;
        cache.insert(module.to_string(), CacheEntry::new(metadata));
    }

    async fn get_module(&self, module: &str, version: &str) -> Option<ModuleInfo> {
        let key = Self::module_key(module, version);
        let cache = self.modules.read().await;
        cache.get(&key).and_then(|entry| {
            if entry.is_expired(self.ttl) {
                None
            } else {
                Some(entry.value.clone())
            }
        })
    }

    async fn set_module(&self, module: &str, version: &str, info: ModuleInfo) {
        let key = Self::module_key(module, version);
        let mut cache = self.modules.write().await;
        cache.insert(key, CacheEntry::new(info));
    }

    async fn get_source(&self, module: &str, version: &str) -> Option<SourceInfo> {
        let key = Self::module_key(module, version);
        let cache = self.sources.read().await;
        cache.get(&key).and_then(|entry| {
            if entry.is_expired(self.ttl) {
                None
            } else {
                Some(entry.value.clone())
            }
        })
    }

    async fn set_source(&self, module: &str, version: &str, source: SourceInfo) {
        let key = Self::module_key(module, version);
        let mut cache = self.sources.write().await;
        cache.insert(key, CacheEntry::new(source));
    }
}

/// Configuration for the registry client.
#[derive(Debug, Clone)]
pub struct RegistryClientConfig {
    /// Connection timeout.
    pub connect_timeout: Duration,
    /// Request timeout.
    pub request_timeout: Duration,
    /// Cache TTL.
    pub cache_ttl: Duration,
    /// User agent string.
    pub user_agent: String,
    /// Maximum number of retries for transient failures.
    pub max_retries: u32,
    /// Delay between retries (with exponential backoff).
    pub retry_delay: Duration,
}

impl Default for RegistryClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            cache_ttl: Duration::from_secs(300), // 5 minutes
            user_agent: format!("rebaze-bzlmod/{}", env!("CARGO_PKG_VERSION")),
            max_retries: 3,
            retry_delay: Duration::from_millis(100),
        }
    }
}

/// HTTP client for Bazel Central Registry.
///
/// This client implements the BCR protocol for fetching module metadata,
/// MODULE.bazel files, and source information. It supports:
///
/// - Connection pooling via reqwest's built-in pool
/// - In-memory caching with configurable TTL
/// - Automatic retries with exponential backoff
/// - Proper error handling and categorization
#[derive(Debug)]
pub struct RegistryClient {
    /// Base URL of the registry.
    base_url: String,
    /// HTTP client with connection pooling.
    client: Client,
    /// In-memory cache.
    cache: Arc<RegistryCache>,
    /// Client configuration.
    config: RegistryClientConfig,
}

impl Clone for RegistryClient {
    fn clone(&self) -> Self {
        Self {
            base_url: self.base_url.clone(),
            client: self.client.clone(),
            cache: Arc::clone(&self.cache),
            config: self.config.clone(),
        }
    }
}

impl RegistryClient {
    /// Create a new registry client with default configuration.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::with_config(base_url, RegistryClientConfig::default())
    }

    /// Create a new registry client with custom configuration.
    pub fn with_config(base_url: impl Into<String>, config: RegistryClientConfig) -> Self {
        let client = Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .user_agent(&config.user_agent)
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(90))
            .gzip(true)
            .build()
            .expect("failed to build HTTP client");

        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            client,
            cache: Arc::new(RegistryCache::new(config.cache_ttl)),
            config,
        }
    }

    /// Create a client for the default BCR.
    #[must_use] 
    pub fn bcr() -> Self {
        Self::new(crate::DEFAULT_REGISTRY)
    }

    /// Create a client for the BCR GitHub mirror.
    #[must_use] 
    pub fn bcr_mirror() -> Self {
        Self::new(crate::DEFAULT_REGISTRY_MIRROR)
    }

    /// Get the base URL of this registry.
    #[must_use] 
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Clear all cached entries.
    pub async fn clear_cache(&self) {
        let mut metadata = self.cache.metadata.write().await;
        let mut modules = self.cache.modules.write().await;
        let mut sources = self.cache.sources.write().await;
        metadata.clear();
        modules.clear();
        sources.clear();
    }

    /// Build a URL for a given path.
    fn build_url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    /// Execute a GET request with retry logic.
    #[instrument(skip(self), fields(url = %url))]
    async fn get_with_retry(&self, url: &str) -> Result<reqwest::Response> {
        let mut last_error = None;
        let mut delay = self.config.retry_delay;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                debug!(attempt, "retrying request after delay");
                tokio::time::sleep(delay).await;
                delay *= 2; // Exponential backoff
            }

            match self.client.get(url).send().await {
                Ok(response) => {
                    let status = response.status();

                    // Don't retry client errors (4xx) except for rate limiting
                    if status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS {
                        return Err(RegistryError::HttpStatus {
                            status: status.as_u16(),
                            url: url.to_string(),
                        }
                        .into());
                    }

                    // Retry server errors (5xx) and rate limiting
                    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
                        last_error = Some(RegistryError::HttpStatus {
                            status: status.as_u16(),
                            url: url.to_string(),
                        });
                        continue;
                    }

                    return Ok(response);
                }
                Err(e) => {
                    // Retry on connection/timeout errors
                    if e.is_connect() || e.is_timeout() {
                        warn!(error = %e, "transient error, will retry");
                        last_error = Some(RegistryError::Http(e));
                        continue;
                    }
                    // Don't retry other errors
                    return Err(RegistryError::Http(e).into());
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| RegistryError::NotReachable(url.to_string()))
            .into())
    }

    /// Fetch and parse JSON from a URL.
    #[instrument(skip(self), fields(url = %url))]
    async fn fetch_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response = self.get_with_retry(url).await?;
        let bytes = response.bytes().await.map_err(RegistryError::Http)?;
        serde_json::from_slice(&bytes).map_err(|e| {
            RegistryError::InvalidMetadata(format!("failed to parse JSON from {url}: {e}")).into()
        })
    }

    /// Fetch raw text from a URL.
    #[instrument(skip(self), fields(url = %url))]
    async fn fetch_text(&self, url: &str) -> Result<String> {
        let response = self.get_with_retry(url).await?;
        response
            .text()
            .await
            .map_err(|e| RegistryError::Http(e).into())
    }

    /// Parse MODULE.bazel content into `ModuleInfo`.
    ///
    /// This is a simplified parser that extracts basic module information.
    /// For full parsing, the `parser` module should be used.
    fn parse_module_bazel(content: &str, module: &str, version: &str) -> Result<ModuleInfo> {
        // Extract module name from content or use provided name
        let name =
            Self::extract_string_field(content, "name").unwrap_or_else(|| module.to_string());

        // Extract version from content or use provided version
        let extracted_version =
            Self::extract_string_field(content, "version").unwrap_or_else(|| version.to_string());

        // Extract compatibility level
        let compatibility_level =
            Self::extract_int_field(content, "compatibility_level").unwrap_or(0);

        // Extract bazel_compatibility
        let bazel_compatibility =
            Self::extract_list_field(content, "bazel_compatibility").unwrap_or_default();

        // Parse dependencies
        let deps = Self::extract_deps(content, false);
        let dev_deps = Self::extract_deps(content, true);

        Ok(ModuleInfo {
            name: ModuleName::new(name)?,
            version: Version::new(extracted_version)?,
            compatibility_level,
            bazel_compatibility,
            deps,
            dev_deps,
            overrides: Vec::new(), // Overrides are typically in root module only
        })
    }

    /// Extract a string field from MODULE.bazel content.
    fn extract_string_field(content: &str, field: &str) -> Option<String> {
        // Match patterns like: name = "value" or name="value"
        let pattern = format!(r#"{field}\s*=\s*"([^"]+)""#);
        regex::Regex::new(&pattern)
            .ok()
            .and_then(|re| re.captures(content))
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }

    /// Extract an integer field from MODULE.bazel content.
    fn extract_int_field(content: &str, field: &str) -> Option<u32> {
        let pattern = format!(r"{field}\s*=\s*(\d+)");
        regex::Regex::new(&pattern)
            .ok()
            .and_then(|re| re.captures(content))
            .and_then(|caps| caps.get(1))
            .and_then(|m| m.as_str().parse().ok())
    }

    /// Extract a list field from MODULE.bazel content.
    fn extract_list_field(content: &str, field: &str) -> Option<Vec<String>> {
        // Match patterns like: field = ["item1", "item2"]
        let pattern = format!(r"{field}\s*=\s*\[([^\]]*)\]");
        regex::Regex::new(&pattern)
            .ok()
            .and_then(|re| re.captures(content))
            .and_then(|caps| caps.get(1))
            .map(|m| {
                let items_str = m.as_str();
                regex::Regex::new(r#""([^"]+)""#)
                    .ok()
                    .map(|re| {
                        re.captures_iter(items_str)
                            .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
                            .collect()
                    })
                    .unwrap_or_default()
            })
    }

    /// Extract `bazel_dep` declarations from MODULE.bazel content.
    fn extract_deps(content: &str, dev_only: bool) -> Vec<crate::Dependency> {
        let mut deps = Vec::new();

        // Match bazel_dep declarations
        // Pattern: bazel_dep(name = "...", version = "...", ...)
        let dep_pattern = r"bazel_dep\s*\(\s*([^)]+)\)";
        let re = match regex::Regex::new(dep_pattern) {
            Ok(r) => r,
            Err(_) => return deps,
        };

        for cap in re.captures_iter(content) {
            let args = cap.get(1).map_or("", |m| m.as_str());

            // Extract name
            let name = match Self::extract_arg_string(args, "name") {
                Some(n) => n,
                None => continue,
            };

            // Extract version
            let version = match Self::extract_arg_string(args, "version") {
                Some(v) => v,
                None => continue,
            };

            // Check if it's a dev dependency
            let is_dev = Self::extract_arg_bool(args, "dev_dependency").unwrap_or(false);

            // Filter based on dev_only flag
            if dev_only != is_dev {
                continue;
            }

            // Extract optional fields
            let max_version =
                Self::extract_arg_string(args, "max_version").and_then(|v| Version::new(v).ok());
            let repo_name = Self::extract_arg_string(args, "repo_name");

            if let (Ok(mod_name), Ok(ver)) = (ModuleName::new(name), Version::new(version)) {
                deps.push(crate::Dependency {
                    name: mod_name,
                    version: ver,
                    max_version,
                    repo_name,
                    dev_dependency: is_dev,
                });
            }
        }

        deps
    }

    /// Extract a string argument from function call arguments.
    fn extract_arg_string(args: &str, arg_name: &str) -> Option<String> {
        let pattern = format!(r#"{arg_name}\s*=\s*"([^"]+)""#);
        regex::Regex::new(&pattern)
            .ok()
            .and_then(|re| re.captures(args))
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }

    /// Extract a boolean argument from function call arguments.
    fn extract_arg_bool(args: &str, arg_name: &str) -> Option<bool> {
        let pattern = format!(r"{arg_name}\s*=\s*(True|False)");
        regex::Regex::new(&pattern)
            .ok()
            .and_then(|re| re.captures(args))
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str() == "True")
    }
}

#[async_trait]
impl Registry for RegistryClient {
    #[instrument(skip(self), fields(registry = %self.base_url))]
    async fn get_metadata(&self, module: &str) -> Result<ModuleMetadata> {
        // Check cache first
        if let Some(cached) = self.cache.get_metadata(module).await {
            debug!(module, "cache hit for metadata");
            return Ok(cached);
        }

        debug!(module, "fetching metadata from registry");
        let url = self.build_url(&format!("modules/{module}/metadata.json"));
        let metadata: ModuleMetadata = self.fetch_json(&url).await?;

        // Cache the result
        self.cache.set_metadata(module, metadata.clone()).await;

        Ok(metadata)
    }

    #[instrument(skip(self), fields(registry = %self.base_url))]
    async fn get_module(&self, module: &str, version: &str) -> Result<ModuleInfo> {
        // Check cache first
        if let Some(cached) = self.cache.get_module(module, version).await {
            debug!(module, version, "cache hit for module info");
            return Ok(cached);
        }

        debug!(module, version, "fetching MODULE.bazel from registry");
        let url = self.build_url(&format!("modules/{module}/{version}/MODULE.bazel"));
        let content = self.fetch_text(&url).await?;

        // Parse the MODULE.bazel content
        let info = Self::parse_module_bazel(&content, module, version)?;

        // Cache the result
        self.cache.set_module(module, version, info.clone()).await;

        Ok(info)
    }

    #[instrument(skip(self), fields(registry = %self.base_url))]
    async fn get_source(&self, module: &str, version: &str) -> Result<SourceInfo> {
        // Check cache first
        if let Some(cached) = self.cache.get_source(module, version).await {
            debug!(module, version, "cache hit for source info");
            return Ok(cached);
        }

        debug!(module, version, "fetching source.json from registry");
        let url = self.build_url(&format!("modules/{module}/{version}/source.json"));
        let source: SourceInfo = self.fetch_json(&url).await?;

        // Cache the result
        self.cache.set_source(module, version, source.clone()).await;

        Ok(source)
    }

    #[instrument(skip(self), fields(registry = %self.base_url))]
    async fn has_module(&self, module: &str) -> Result<bool> {
        // Try to get metadata - if it succeeds, the module exists
        match self.get_metadata(module).await {
            Ok(_) => Ok(true),
            Err(e) => {
                // Check if this is a 404 error
                if let crate::Error::Registry(RegistryError::HttpStatus { status: 404, .. }) = &e {
                    return Ok(false);
                }
                Err(e)
            }
        }
    }
}

/// Chain of registries with fallback behavior.
///
/// Tries each registry in order until one succeeds.
#[derive(Debug, Clone)]
pub struct RegistryChain {
    registries: Vec<Arc<dyn Registry>>,
}

impl RegistryChain {
    /// Create a new registry chain.
    #[must_use] 
    pub fn new(registries: Vec<Arc<dyn Registry>>) -> Self {
        Self { registries }
    }

    /// Create default chain (BCR + mirror).
    #[must_use] 
    pub fn default_chain() -> Self {
        Self::new(vec![
            Arc::new(RegistryClient::bcr()),
            Arc::new(RegistryClient::bcr_mirror()),
        ])
    }

    /// Add a registry to the chain.
    pub fn with_registry(mut self, registry: Arc<dyn Registry>) -> Self {
        self.registries.push(registry);
        self
    }

    /// Prepend a registry to the chain (highest priority).
    pub fn with_priority_registry(mut self, registry: Arc<dyn Registry>) -> Self {
        self.registries.insert(0, registry);
        self
    }
}

#[async_trait]
impl Registry for RegistryChain {
    async fn get_metadata(&self, module: &str) -> Result<ModuleMetadata> {
        let mut last_error = None;

        for registry in &self.registries {
            match registry.get_metadata(module).await {
                Ok(metadata) => return Ok(metadata),
                Err(e) => {
                    debug!(module, error = %e, "registry failed, trying next");
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| crate::Error::ModuleNotFound(module.to_string())))
    }

    async fn get_module(&self, module: &str, version: &str) -> Result<ModuleInfo> {
        let mut last_error = None;

        for registry in &self.registries {
            match registry.get_module(module, version).await {
                Ok(info) => return Ok(info),
                Err(e) => {
                    debug!(module, version, error = %e, "registry failed, trying next");
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| crate::Error::VersionNotFound {
            module: module.to_string(),
            version: version.to_string(),
        }))
    }

    async fn get_source(&self, module: &str, version: &str) -> Result<SourceInfo> {
        let mut last_error = None;

        for registry in &self.registries {
            match registry.get_source(module, version).await {
                Ok(source) => return Ok(source),
                Err(e) => {
                    debug!(module, version, error = %e, "registry failed, trying next");
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| crate::Error::VersionNotFound {
            module: module.to_string(),
            version: version.to_string(),
        }))
    }

    async fn has_module(&self, module: &str) -> Result<bool> {
        for registry in &self.registries {
            match registry.has_module(module).await {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(e) => {
                    debug!(module, error = %e, "registry failed, trying next");
                }
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_get_metadata() {
        let mock_server = MockServer::start().await;

        let metadata = ModuleMetadata {
            versions: vec!["1.0.0".to_string(), "2.0.0".to_string()],
            yanked_versions: std::collections::BTreeMap::new(),
            deprecated: None,
            maintainers: vec![],
            homepage: Some("https://example.com".to_string()),
        };

        Mock::given(method("GET"))
            .and(path("/modules/rules_rust/metadata.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&metadata))
            .mount(&mock_server)
            .await;

        let client = RegistryClient::new(mock_server.uri());
        let result = client.get_metadata("rules_rust").await.unwrap();

        assert_eq!(result.versions, vec!["1.0.0", "2.0.0"]);
        assert_eq!(result.homepage, Some("https://example.com".to_string()));
    }

    #[tokio::test]
    async fn test_get_metadata_cached() {
        let mock_server = MockServer::start().await;

        let metadata = ModuleMetadata {
            versions: vec!["1.0.0".to_string()],
            ..Default::default()
        };

        Mock::given(method("GET"))
            .and(path("/modules/rules_rust/metadata.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&metadata))
            .expect(1) // Should only be called once due to caching
            .mount(&mock_server)
            .await;

        let client = RegistryClient::new(mock_server.uri());

        // First call - hits the server
        let result1 = client.get_metadata("rules_rust").await.unwrap();
        assert_eq!(result1.versions, vec!["1.0.0"]);

        // Second call - should use cache
        let result2 = client.get_metadata("rules_rust").await.unwrap();
        assert_eq!(result2.versions, vec!["1.0.0"]);
    }

    #[tokio::test]
    async fn test_get_module() {
        let mock_server = MockServer::start().await;

        let module_content = r#"
module(
    name = "rules_rust",
    version = "0.40.0",
    compatibility_level = 1,
)

bazel_dep(name = "platforms", version = "0.0.8")
bazel_dep(name = "bazel_skylib", version = "1.5.0")
"#;

        Mock::given(method("GET"))
            .and(path("/modules/rules_rust/0.40.0/MODULE.bazel"))
            .respond_with(ResponseTemplate::new(200).set_body_string(module_content))
            .mount(&mock_server)
            .await;

        let client = RegistryClient::new(mock_server.uri());
        let result = client.get_module("rules_rust", "0.40.0").await.unwrap();

        assert_eq!(result.name.as_str(), "rules_rust");
        assert_eq!(result.version.as_str(), "0.40.0");
        assert_eq!(result.compatibility_level, 1);
        assert_eq!(result.deps.len(), 2);
        assert_eq!(result.deps[0].name.as_str(), "platforms");
        assert_eq!(result.deps[0].version.as_str(), "0.0.8");
    }

    #[tokio::test]
    async fn test_get_source() {
        let mock_server = MockServer::start().await;

        let source = SourceInfo {
            source_type: "archive".to_string(),
            url: Some("https://github.com/bazelbuild/rules_rust/releases/download/0.40.0/rules_rust-v0.40.0.tar.gz".to_string()),
            integrity: Some("sha256-abc123".to_string()),
            strip_prefix: Some("rules_rust-0.40.0".to_string()),
            patches: std::collections::BTreeMap::new(),
            patch_strip: 0,
        };

        Mock::given(method("GET"))
            .and(path("/modules/rules_rust/0.40.0/source.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&source))
            .mount(&mock_server)
            .await;

        let client = RegistryClient::new(mock_server.uri());
        let result = client.get_source("rules_rust", "0.40.0").await.unwrap();

        assert_eq!(result.source_type, "archive");
        assert!(result.url.unwrap().contains("rules_rust"));
    }

    #[tokio::test]
    async fn test_has_module() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/modules/rules_rust/metadata.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ModuleMetadata::default()))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/modules/nonexistent/metadata.json"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let client = RegistryClient::new(mock_server.uri());

        assert!(client.has_module("rules_rust").await.unwrap());
        assert!(!client.has_module("nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn test_http_error_handling() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/modules/error_module/metadata.json"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let config = RegistryClientConfig {
            max_retries: 0, // Disable retries for faster test
            ..Default::default()
        };
        let client = RegistryClient::with_config(mock_server.uri(), config);
        let result = client.get_metadata("error_module").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_registry_chain_fallback() {
        let mock_server1 = MockServer::start().await;
        let mock_server2 = MockServer::start().await;

        // First server returns 404
        Mock::given(method("GET"))
            .and(path("/modules/rules_rust/metadata.json"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server1)
            .await;

        // Second server returns success
        let metadata = ModuleMetadata {
            versions: vec!["1.0.0".to_string()],
            ..Default::default()
        };
        Mock::given(method("GET"))
            .and(path("/modules/rules_rust/metadata.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&metadata))
            .mount(&mock_server2)
            .await;

        let chain = RegistryChain::new(vec![
            Arc::new(RegistryClient::new(mock_server1.uri())),
            Arc::new(RegistryClient::new(mock_server2.uri())),
        ]);

        let result = chain.get_metadata("rules_rust").await.unwrap();
        assert_eq!(result.versions, vec!["1.0.0"]);
    }

    #[test]
    fn test_parse_module_bazel_basic() {
        let content = r#"
module(
    name = "test_module",
    version = "1.2.3",
)
"#;

        let info = RegistryClient::parse_module_bazel(content, "test_module", "1.2.3").unwrap();
        assert_eq!(info.name.as_str(), "test_module");
        assert_eq!(info.version.as_str(), "1.2.3");
    }

    #[test]
    fn test_parse_module_bazel_with_deps() {
        let content = r#"
module(
    name = "my_module",
    version = "2.0.0",
    compatibility_level = 2,
)

bazel_dep(name = "dep1", version = "1.0.0")
bazel_dep(name = "dep2", version = "2.0.0", dev_dependency = True)
"#;

        let info = RegistryClient::parse_module_bazel(content, "my_module", "2.0.0").unwrap();
        assert_eq!(info.name.as_str(), "my_module");
        assert_eq!(info.version.as_str(), "2.0.0");
        assert_eq!(info.compatibility_level, 2);
        assert_eq!(info.deps.len(), 1);
        assert_eq!(info.deps[0].name.as_str(), "dep1");
        assert_eq!(info.dev_deps.len(), 1);
        assert_eq!(info.dev_deps[0].name.as_str(), "dep2");
    }

    #[test]
    fn test_extract_string_field() {
        let content = r#"name = "test_value""#;
        assert_eq!(
            RegistryClient::extract_string_field(content, "name"),
            Some("test_value".to_string())
        );

        let content2 = r#"name="no_spaces""#;
        assert_eq!(
            RegistryClient::extract_string_field(content2, "name"),
            Some("no_spaces".to_string())
        );
    }

    #[test]
    fn test_extract_int_field() {
        let content = "compatibility_level = 42";
        assert_eq!(
            RegistryClient::extract_int_field(content, "compatibility_level"),
            Some(42)
        );
    }

    #[test]
    fn test_extract_list_field() {
        let content = r#"bazel_compatibility = [">=7.0.0", "<8.0.0"]"#;
        let result = RegistryClient::extract_list_field(content, "bazel_compatibility");
        assert_eq!(
            result,
            Some(vec![">=7.0.0".to_string(), "<8.0.0".to_string()])
        );
    }
}
