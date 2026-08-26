//! The registry: which plugins and commands exist, and which are disabled.
//!
//! Everything here answers "what is available"; `invoke` answers "run it".

use super::*;

pub struct PluginManager {
    pub(super) dynamic_plugins: HashMap<String, DynamicPlugin>,
    pub(super) builtin_plugins: HashMap<String, Arc<dyn BuiltinPlugin>>,
    pub(super) host_plugins: HashMap<String, Arc<dyn BuiltinPlugin>>,
    pub(super) reserved_dynamic_domains: HashSet<String>,
    pub(super) disabled_domains: RwLock<HashSet<String>>,
    pub(super) typed_registry: RwLock<Option<Arc<TypedRegistry>>>,
    pub(super) catalog_revision: AtomicU64,
    pub(super) registry_build_count: AtomicU64,
    pub(super) secret_resolver: Option<Arc<dyn SecretResolver>>,
}

#[derive(Debug, Clone)]
pub struct RegisteredPlugin {
    pub metadata: PluginMetadata,
    pub source: PluginSource,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct RegisteredCommand {
    pub descriptor: CommandDescriptor,
    pub plugin: PluginMetadata,
    pub source: PluginSource,
}

pub(super) struct TypedRegistry {
    pub(super) commands: Vec<Arc<RegisteredTypedCommand>>,
    pub(super) by_id: HashMap<String, Arc<RegisteredTypedCommand>>,
}

pub(super) struct RegisteredTypedCommand {
    pub(super) registered: RegisteredCommand,
    pub(super) route: TypedCommandRoute,
    pub(super) input_validator: jsonschema::Validator,
    pub(super) output_validator: jsonschema::Validator,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TypedCommandRoute {
    Host,
    Builtin,
    Dynamic,
}

pub(super) fn append_typed_catalog(
    commands: &mut Vec<Arc<RegisteredTypedCommand>>,
    metadata: PluginMetadata,
    source: PluginSource,
    route: TypedCommandRoute,
    catalog: CommandCatalog,
) -> Result<(), RuntimeError> {
    let validators = typed::compile_catalog(&metadata, &catalog)?;
    for (descriptor, validators) in catalog.commands.into_iter().zip(validators) {
        debug_assert_eq!(descriptor.id, validators.command_id);
        commands.push(Arc::new(RegisteredTypedCommand {
            registered: RegisteredCommand {
                descriptor,
                plugin: metadata.clone(),
                source,
            },
            route,
            input_validator: validators.input,
            output_validator: validators.output,
        }));
    }
    Ok(())
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) fn push_dynamic_plugin_conflicts(
    report: &mut PluginLoadReport,
    domain_key: &str,
    new_metadata: &PluginMetadata,
    existing_dynamic: Option<&PluginMetadata>,
    existing_builtin: Option<PluginMetadata>,
) {
    if let Some(existing) = existing_dynamic
        && !is_same_plugin_identity(new_metadata, existing)
    {
        report.push_conflict(
            new_metadata.clone(),
            existing.clone(),
            PluginSource::Dynamic,
            PluginSource::Dynamic,
            format!(
                "multiple dynamic plugins for domain '{domain_key}', last loaded takes precedence",
            ),
        );
    }
    if let Some(existing) = existing_builtin {
        report.push_conflict(
            new_metadata.clone(),
            existing,
            PluginSource::Dynamic,
            PluginSource::Builtin,
            format!("dynamic plugin shadows builtin plugin for domain '{domain_key}'"),
        );
    }
}

pub(super) fn is_same_plugin_identity(left: &PluginMetadata, right: &PluginMetadata) -> bool {
    left.plugin_name == right.plugin_name
        && left.domain.eq_ignore_ascii_case(&right.domain)
        && left.description == right.description
        && left.abi_version == right.abi_version
        && left.compatibility == right.compatibility
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TypedSymbolAvailability {
    pub(super) catalog: bool,
    pub(super) invoke: bool,
    pub(super) cancel: bool,
}

impl TypedSymbolAvailability {
    pub(super) fn is_complete(self) -> bool {
        self.catalog && self.invoke && self.cancel
    }

    pub(super) fn any(self) -> bool {
        self.catalog || self.invoke || self.cancel
    }
}

pub(super) fn read_disabled_domains(
    domains: &RwLock<HashSet<String>>,
) -> std::sync::RwLockReadGuard<'_, HashSet<String>> {
    domains
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(super) fn write_disabled_domains(
    domains: &RwLock<HashSet<String>>,
) -> std::sync::RwLockWriteGuard<'_, HashSet<String>> {
    domains
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            dynamic_plugins: HashMap::new(),
            builtin_plugins: HashMap::new(),
            host_plugins: HashMap::new(),
            reserved_dynamic_domains: HashSet::new(),
            disabled_domains: RwLock::new(HashSet::new()),
            typed_registry: RwLock::new(None),
            catalog_revision: AtomicU64::new(1),
            registry_build_count: AtomicU64::new(0),
            secret_resolver: None,
        }
    }

    pub fn set_secret_resolver(&mut self, resolver: Arc<dyn SecretResolver>) {
        self.secret_resolver = Some(resolver);
    }

    pub fn set_disabled_domains<I>(&self, domains: I)
    where
        I: IntoIterator<Item = String>,
    {
        let domains: HashSet<String> = domains
            .into_iter()
            .map(|domain| domain_key(&domain))
            .collect();
        let mut disabled_domains = write_disabled_domains(&self.disabled_domains);
        if *disabled_domains != domains {
            *disabled_domains = domains;
            self.catalog_revision.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub fn is_domain_disabled(&self, domain: &str) -> bool {
        read_disabled_domains(&self.disabled_domains).contains(&domain_key(domain))
    }

    pub fn reserve_dynamic_domains<I, S>(&mut self, domains: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.reserved_dynamic_domains.extend(
            domains
                .into_iter()
                .map(|domain| domain_key(domain.as_ref())),
        );
    }

    pub fn register_builtin(&mut self, plugin: Arc<dyn BuiltinPlugin>) {
        let key = domain_key(&plugin.metadata().domain);
        self.builtin_plugins.insert(key, plugin);
        self.invalidate_typed_registry();
    }

    pub fn register_host_builtin(&mut self, plugin: Arc<dyn BuiltinPlugin>) {
        let key = domain_key(&plugin.metadata().domain);
        self.host_plugins.insert(key, plugin);
        self.invalidate_typed_registry();
    }

    pub fn load_dynamic_plugins_from_dir(&mut self, dir: &Path) -> PluginLoadReport {
        let mut report = PluginLoadReport::default();
        if !dir.exists() {
            return report;
        }
        let entries = match fs::read_dir(dir) {
            Ok(value) => value,
            Err(error) => {
                report.push_warning(
                    dir.to_path_buf(),
                    format!("failed to read plugin directory: {error}"),
                );
                return report;
            }
        };

        let mut plugin_paths = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(value) => value,
                Err(error) => {
                    report.push_warning(
                        dir.to_path_buf(),
                        format!("failed to read plugin directory entry: {error}"),
                    );
                    continue;
                }
            };
            let path = entry.path();
            if !path.is_file() || !is_dynamic_lib_file(&path) {
                continue;
            }
            plugin_paths.push(path);
        }

        plugin_paths.sort_unstable_by(|left, right| left.file_name().cmp(&right.file_name()));
        let mut definitions_changed = false;
        for path in plugin_paths {
            match DynamicPlugin::load(path.clone()) {
                Ok(plugin) => {
                    let domain_key = domain_key(&plugin.metadata.domain);
                    if self.reserved_dynamic_domains.contains(&domain_key) {
                        report.push_warning(
                            path,
                            format!("dynamic plugin domain '{domain_key}' is reserved by the host"),
                        );
                        continue;
                    }
                    push_dynamic_plugin_conflicts(
                        &mut report,
                        &domain_key,
                        &plugin.metadata,
                        self.dynamic_plugins
                            .get(&domain_key)
                            .map(|existing| &existing.metadata),
                        self.builtin_plugins
                            .get(&domain_key)
                            .map(|existing| existing.metadata()),
                    );

                    self.dynamic_plugins.insert(domain_key, plugin);
                    report.loaded += 1;
                    definitions_changed = true;
                }
                Err(error) => {
                    report.push_warning(path, error.to_string());
                }
            }
        }

        if definitions_changed {
            self.invalidate_typed_registry();
        }

        report
    }

    pub fn list_enabled_commands(&self) -> Result<Vec<RegisteredCommand>, RuntimeError> {
        let registry = self.typed_registry()?;
        let disabled_domains = read_disabled_domains(&self.disabled_domains).clone();
        Ok(registry
            .commands
            .iter()
            .filter(|command| {
                command.route == TypedCommandRoute::Host
                    || !disabled_domains.contains(&domain_key(&command.registered.plugin.domain))
            })
            .map(|command| command.registered.clone())
            .collect())
    }

    pub fn catalog_revision(&self) -> u64 {
        self.catalog_revision.load(Ordering::Acquire)
    }

    pub fn list_plugins(&self) -> Vec<PluginMetadata> {
        self.list_registered_plugins()
            .into_iter()
            .map(|plugin| plugin.metadata)
            .collect()
    }

    pub fn list_registered_plugins(&self) -> Vec<RegisteredPlugin> {
        let mut plugins_by_domain = HashMap::new();
        for plugin in self.builtin_plugins.values() {
            let metadata = plugin.metadata();
            let domain = domain_key(&metadata.domain);
            plugins_by_domain.entry(domain).or_insert(RegisteredPlugin {
                enabled: !self.is_domain_disabled(&metadata.domain),
                metadata,
                source: PluginSource::Builtin,
            });
        }
        for plugin in self.dynamic_plugins.values() {
            let metadata = plugin.metadata.clone();
            let domain = domain_key(&metadata.domain);
            plugins_by_domain.insert(
                domain,
                RegisteredPlugin {
                    enabled: !self.is_domain_disabled(&metadata.domain),
                    metadata,
                    source: PluginSource::Dynamic,
                },
            );
        }
        let mut plugins = Vec::from_iter(plugins_by_domain.into_values());
        plugins.sort_by(|left, right| {
            left.metadata
                .domain
                .cmp(&right.metadata.domain)
                .then_with(|| left.metadata.plugin_name.cmp(&right.metadata.plugin_name))
        });
        plugins
    }

    pub fn collect_plugin_manuals(&self) -> Vec<PluginManual> {
        let mut manuals = Vec::new();
        for plugin in self.list_registered_plugins() {
            if !plugin.enabled {
                continue;
            }
            let domain = domain_key(&plugin.metadata.domain);
            match plugin.source {
                PluginSource::Builtin => {
                    let plugin = self
                        .builtin_plugins
                        .get(&domain)
                        .expect("builtin plugin should exist for listed domain");
                    manuals.push(plugin.manual());
                }
                PluginSource::Dynamic => {
                    let plugin = self
                        .dynamic_plugins
                        .get(&domain)
                        .expect("dynamic plugin should exist for listed domain");
                    match plugin.manual() {
                        Ok(Some(manual)) => manuals.push(manual),
                        Ok(None) => manuals.push(fallback_manual(
                            &plugin.metadata,
                            "manual is not provided by this dynamic plugin".to_owned(),
                        )),
                        Err(error) => manuals.push(fallback_manual(
                            &plugin.metadata,
                            format!("failed to load plugin manual: {error}"),
                        )),
                    }
                }
            }
        }
        manuals.sort_by(|left, right| left.domain.cmp(&right.domain));
        manuals
    }

    pub fn list_enabled_plugins(&self) -> Vec<PluginMetadata> {
        self.list_registered_plugins()
            .into_iter()
            .filter(|plugin| plugin.enabled)
            .map(|plugin| plugin.metadata)
            .collect()
    }
}
