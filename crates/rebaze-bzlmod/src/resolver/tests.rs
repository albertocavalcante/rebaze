#![allow(clippy::unwrap_used)]

use super::*;
use std::time::Duration;

#[test]
fn test_version_comparison() {
    assert!(compare_versions("1.0.0", "0.9.0") > 0);
    assert_eq!(compare_versions("1.0.0", "1.0.0"), 0);
    assert!(compare_versions("1.0.0", "1.0.1") < 0);
    assert!(compare_versions("2.0.0", "1.9.9") > 0);
    assert!(compare_versions("1.0.0-alpha", "1.0.0") < 0);
    assert!(compare_versions("1.0.0-alpha", "1.0.0-beta") < 0);
}

#[test]
fn test_parse_module_content() {
    let content = r#"
module(name = "my_project", version = "1.0.0")
bazel_dep(name = "rules_rust", version = "0.40.0")
bazel_dep(name = "rules_go", version = "0.50.0", dev_dependency = True)
    "#;

    let info = parse_module_content(content).unwrap();
    assert_eq!(info.name.as_str(), "my_project");
    assert_eq!(info.version.as_str(), "1.0.0");
    assert_eq!(info.deps.len(), 1);
    assert_eq!(info.deps[0].name.as_str(), "rules_rust");
    assert_eq!(info.dev_deps.len(), 1);
    assert_eq!(info.dev_deps[0].name.as_str(), "rules_go");
}

#[test]
fn test_resolver_options_default() {
    let options = ResolverOptions::default();
    assert_eq!(options.concurrency, DEFAULT_MAX_CONCURRENCY);
    assert_eq!(options.max_depth, DEFAULT_MAX_DEPTH);
    assert!(!options.include_dev_deps);
    assert!(options.check_yanked);
}

#[derive(Debug, Default)]
struct MemoryRegistry {
    modules: HashMap<String, ModuleInfo>,
    metadata: HashMap<String, crate::ModuleMetadata>,
}

impl MemoryRegistry {
    fn from_modules(modules: Vec<ModuleInfo>) -> Self {
        Self {
            modules: modules
                .into_iter()
                .map(|module| (format!("{}@{}", module.name, module.version), module))
                .collect(),
            ..Self::default()
        }
    }
}

#[async_trait::async_trait]
impl Registry for MemoryRegistry {
    async fn get_metadata(&self, module: &str) -> Result<crate::ModuleMetadata> {
        Ok(self.metadata.get(module).cloned().unwrap_or_default())
    }
    async fn get_module(&self, module: &str, version: &str) -> Result<ModuleInfo> {
        assert_ne!(module, "panic", "registry task panicked");
        if module == "pending" {
            return std::future::pending().await;
        }
        self.modules
            .get(&format!("{module}@{version}"))
            .cloned()
            .ok_or_else(|| Error::VersionNotFound {
                module: module.into(),
                version: version.into(),
            })
    }
    async fn get_source(&self, module: &str, _version: &str) -> Result<crate::SourceInfo> {
        Err(Error::ModuleNotFound(module.into()))
    }
    async fn has_module(&self, module: &str) -> Result<bool> {
        Ok(self
            .modules
            .values()
            .any(|info| info.name.as_str() == module))
    }
}

fn module(name: &str, deps: &[&str]) -> ModuleInfo {
    let mut info = ModuleInfo {
        name: ModuleName::new(name).unwrap(),
        version: Version::new("1.0").unwrap(),
        compatibility_level: 0,
        bazel_compatibility: vec![],
        deps: vec![],
        dev_deps: vec![],
        overrides: vec![],
    };
    info.deps = deps
        .iter()
        .map(|name| Dependency {
            name: ModuleName::new(*name).unwrap(),
            version: Version::new("1.0").unwrap(),
            max_version: None,
            repo_name: None,
            dev_dependency: false,
        })
        .collect();
    info
}

fn resolver(registry: MemoryRegistry) -> Resolver<MemoryRegistry> {
    Resolver::new(
        registry,
        ResolverOptions {
            check_yanked: false,
            ..ResolverOptions::default()
        },
    )
}

#[tokio::test]
async fn fetches_full_transitive_closure_and_connects_root() {
    let root = module("root", &["a"]);
    let result = resolver(MemoryRegistry::from_modules(vec![
        module("a", &["b"]),
        module("b", &["c"]),
        module("c", &[]),
    ]))
    .resolve_dependencies(&root)
    .await
    .unwrap();
    assert_eq!(
        result
            .modules
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert_eq!(result.stats.registry_fetches, 3);
    assert_eq!(result.stats.direct_deps, 1);
    assert_eq!(result.stats.transitive_deps, 2);
    assert_eq!(
        result.graph.direct_deps(result.graph.root().unwrap())[0]
            .name
            .as_str(),
        "a"
    );
    let (a, _) = result.graph.get_by_name("a").unwrap();
    assert_eq!(result.graph.direct_deps(a)[0].name.as_str(), "b");
}

#[tokio::test]
async fn a_wide_root_does_not_deadlock_before_workers_start() {
    let names: Vec<_> = (0..150).map(|i| format!("dep{i}")).collect();
    let names: Vec<_> = names.iter().map(String::as_str).collect();
    let root = module("root", &names);
    let registry =
        MemoryRegistry::from_modules(names.iter().map(|name| module(name, &[])).collect());
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        resolver(registry).resolve_dependencies(&root),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.modules.len(), 150);
}

#[tokio::test]
async fn missing_transitive_modules_fail_instead_of_producing_partial_success() {
    let registry = MemoryRegistry::from_modules(vec![module("a", &["missing"])]);
    let result = resolver(registry)
        .resolve_dependencies(&module("root", &["a"]))
        .await;
    assert!(matches!(result, Err(Error::VersionNotFound { module, .. }) if module == "missing"));
}

#[tokio::test]
async fn zero_concurrency_and_depth_overflow_are_errors() {
    let mut resolver = resolver(MemoryRegistry::from_modules(vec![
        module("a", &["b"]),
        module("b", &[]),
    ]));
    resolver.options.concurrency = 0;
    assert!(
        resolver
            .resolve_dependencies(&module("root", &["a"]))
            .await
            .is_err()
    );
    resolver.options.concurrency = 1;
    resolver.options.max_depth = 1;
    assert!(matches!(
        resolver.resolve_dependencies(&module("root", &["a"])).await,
        Err(Error::MaxDepthExceeded(1))
    ));
}

#[tokio::test]
async fn fetch_panics_and_timeouts_are_reported() {
    for name in ["panic", "pending"] {
        let mut resolver = resolver(MemoryRegistry::default());
        resolver.options.timeout_secs = 0;
        assert!(matches!(
            resolver
                .resolve_dependencies(&module("root", &[name]))
                .await,
            Err(Error::Resolution(_))
        ));
    }
}

#[tokio::test]
async fn shared_dependencies_are_fetched_once_and_transitive_dev_deps_are_ignored() {
    let mut a = module("a", &["shared"]);
    a.dev_deps = module("unused", &["missing_dev"]).deps;
    let mut root = module("root", &["a", "b"]);
    root.dev_deps = module("unused", &["shared"]).deps;
    let mut resolver = resolver(MemoryRegistry::from_modules(vec![
        a,
        module("b", &["shared"]),
        module("shared", &[]),
    ]));
    resolver.options.include_dev_deps = true;
    let result = resolver.resolve_dependencies(&root).await.unwrap();
    assert_eq!(result.stats.registry_fetches, 3);
    assert!(
        !result
            .modules
            .iter()
            .find(|m| m.name.as_str() == "shared")
            .unwrap()
            .is_dev
    );
}

#[tokio::test]
async fn selected_version_controls_edges_and_prunes_unreachable_modules() {
    let mut a2 = module("a", &["new"]);
    a2.version = Version::new("2.0").unwrap();
    let mut b = module("b", &["a"]);
    b.deps[0].version = Version::new("2.0").unwrap();
    let result = resolver(MemoryRegistry::from_modules(vec![
        module("a", &["old"]),
        a2,
        b,
        module("old", &[]),
        module("new", &[]),
    ]))
    .resolve_dependencies(&module("root", &["a", "b"]))
    .await
    .unwrap();
    assert!(!result.graph.contains_name("old"));
    let (a, _) = result.graph.get_by_name("a").unwrap();
    assert_eq!(a.version.as_str(), "2.0");
    assert_eq!(result.graph.direct_deps(a)[0].name.as_str(), "new");
}

#[tokio::test]
async fn local_overrides_hydrate_transitive_dependencies_without_registry_fetch() {
    let mut root = module("root", &["a"]);
    root.overrides.push(Override::LocalPath {
        module: ModuleName::new("b").unwrap(),
        path: "local".into(),
    });
    let resolver = resolver(MemoryRegistry::from_modules(vec![
        module("a", &["b"]),
        module("c", &["d"]),
        module("d", &[]),
    ]));
    resolver
        .add_override_module_info("b", module("b", &["c"]))
        .await
        .unwrap();
    let result = resolver.resolve_dependencies(&root).await.unwrap();
    assert_eq!(result.modules.len(), 4);
    assert_eq!(result.stats.registry_fetches, 3);
}

#[tokio::test]
async fn yanked_substitution_chooses_lowest_candidate_and_fetches_its_dependencies() {
    let mut replacement = module("a", &["b"]);
    replacement.version = Version::new("1.1").unwrap();
    let mut registry = MemoryRegistry::from_modules(vec![replacement, module("b", &[])]);
    registry.metadata.insert(
        "a".into(),
        crate::ModuleMetadata {
            versions: vec!["3.0".into(), "1.1".into(), "2.0".into()],
            yanked_versions: [("1.0".into(), "bug".into())].into(),
            ..crate::ModuleMetadata::default()
        },
    );
    let mut resolver = resolver(registry);
    resolver.options.substitute_yanked = true;
    let result = resolver
        .resolve_dependencies(&module("root", &["a"]))
        .await
        .unwrap();
    assert_eq!(result.modules[0].version.as_str(), "1.1");
    assert!(result.graph.contains_name("b"));
}

#[test]
fn module_parser_preserves_overrides_and_rejects_malformed_input() {
    let info = parse_module_content("module(version = '1.0', name = 'root')\nlocal_path_override(path = 'local', module_name = 'dep')").unwrap();
    assert_eq!(info.overrides.len(), 1);
    assert!(parse_module_content("# module(name = 'fake', version = '1.0')").is_err());
    assert!(parse_module_content("module(").is_err());
}

#[tokio::test]
async fn root_cycles_do_not_fetch_the_unpublished_root() {
    let mut a = module("a", &["root"]);
    a.deps[0].version = Version::new("99.0").unwrap();
    let result = resolver(MemoryRegistry::from_modules(vec![a]))
        .resolve_dependencies(&module("root", &["a"]))
        .await
        .unwrap();
    assert_eq!(result.modules.len(), 1);
    assert_eq!(result.stats.registry_fetches, 1);
}

#[tokio::test]
async fn unhydrated_local_override_fails_explicitly() {
    let mut root = module("root", &["a"]);
    root.overrides.push(Override::LocalPath {
        module: ModuleName::new("a").unwrap(),
        path: "local".into(),
    });
    assert!(
        resolver(MemoryRegistry::default())
            .resolve_dependencies(&root)
            .await
            .is_err()
    );
}

#[test]
fn a_single_version_override_without_version_is_not_a_zero_version_pin() {
    assert!(parse_module_content("module(name = 'root')\nsingle_version_override(module_name = 'dep', registry = 'https://example.com')").is_err());
}

#[tokio::test]
async fn metadata_timeouts_are_errors_in_validation_and_substitution() {
    #[derive(Debug)]
    struct PendingMetadata;
    #[async_trait::async_trait]
    impl Registry for PendingMetadata {
        async fn get_metadata(&self, _: &str) -> Result<crate::ModuleMetadata> {
            std::future::pending().await
        }
        async fn get_module(&self, name: &str, _: &str) -> Result<ModuleInfo> {
            Ok(module(name, &[]))
        }
        async fn get_source(&self, name: &str, _: &str) -> Result<crate::SourceInfo> {
            Err(Error::ModuleNotFound(name.into()))
        }
        async fn has_module(&self, _: &str) -> Result<bool> {
            Ok(true)
        }
    }
    for substitute_yanked in [false, true] {
        let resolver = Resolver::new(
            PendingMetadata,
            ResolverOptions {
                timeout_secs: 0,
                substitute_yanked,
                ..ResolverOptions::default()
            },
        );
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            resolver.resolve_dependencies(&module("root", &["a"])),
        )
        .await
        .unwrap();
        assert!(matches!(result, Err(Error::Resolution(_))));
    }
}

#[tokio::test]
async fn local_overrides_do_not_require_registry_metadata() {
    #[derive(Debug)]
    struct OfflineRegistry;
    #[async_trait::async_trait]
    impl Registry for OfflineRegistry {
        async fn get_metadata(&self, name: &str) -> Result<crate::ModuleMetadata> {
            Err(Error::ModuleNotFound(name.into()))
        }
        async fn get_module(&self, name: &str, _: &str) -> Result<ModuleInfo> {
            Err(Error::ModuleNotFound(name.into()))
        }
        async fn get_source(&self, name: &str, _: &str) -> Result<crate::SourceInfo> {
            Err(Error::ModuleNotFound(name.into()))
        }
        async fn has_module(&self, _: &str) -> Result<bool> {
            Ok(false)
        }
    }
    let mut root = module("root", &["a"]);
    root.overrides.push(Override::LocalPath {
        module: ModuleName::new("a").unwrap(),
        path: "local".into(),
    });
    let resolver = Resolver::new(OfflineRegistry, ResolverOptions::default());
    resolver
        .add_override_module_info("a", module("a", &[]))
        .await
        .unwrap();
    assert_eq!(
        resolver
            .resolve_dependencies(&root)
            .await
            .unwrap()
            .modules
            .len(),
        1
    );
}

#[tokio::test]
async fn depth_limit_uses_shortest_paths_despite_slow_shallow_dependency() {
    #[derive(Debug)]
    struct SlowRegistry(MemoryRegistry);
    #[async_trait::async_trait]
    impl Registry for SlowRegistry {
        async fn get_metadata(&self, name: &str) -> Result<crate::ModuleMetadata> {
            self.0.get_metadata(name).await
        }
        async fn get_module(&self, name: &str, version: &str) -> Result<ModuleInfo> {
            if name == "b" {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            self.0.get_module(name, version).await
        }
        async fn get_source(&self, name: &str, version: &str) -> Result<crate::SourceInfo> {
            self.0.get_source(name, version).await
        }
        async fn has_module(&self, name: &str) -> Result<bool> {
            self.0.has_module(name).await
        }
    }
    let registry = SlowRegistry(MemoryRegistry::from_modules(vec![
        module("a", &["c"]),
        module("b", &["d"]),
        module("c", &["d"]),
        module("d", &[]),
    ]));
    let resolver = Resolver::new(
        registry,
        ResolverOptions {
            max_depth: 2,
            check_yanked: false,
            ..ResolverOptions::default()
        },
    );
    assert_eq!(
        resolver
            .resolve_dependencies(&module("root", &["a", "b"]))
            .await
            .unwrap()
            .modules
            .len(),
        4
    );
}
