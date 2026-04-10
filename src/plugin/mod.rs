use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use clap::{Arg, ArgAction, Command};
use semver::{Version, VersionReq};
use toml::Value;

pub use crate::support::{PluginAssetId, PluginId, PluginScaffoldId};

use crate::cli::{CommandInvocation, CommandRegistrar};
use crate::foundation::{AppContext, Error, Result, ServiceProvider};
use crate::http::RouteRegistrar;
use crate::scheduler::ScheduleRegistrar;
use crate::support::ValidationRuleId;
use crate::validation::ValidationRule;
use crate::websocket::WebSocketRouteRegistrar;

const PLUGIN_LIST_COMMAND: crate::support::CommandId =
    crate::support::CommandId::new("plugin:list");
const PLUGIN_INSTALL_ASSETS_COMMAND: crate::support::CommandId =
    crate::support::CommandId::new("plugin:install-assets");
const PLUGIN_SCAFFOLD_COMMAND: crate::support::CommandId =
    crate::support::CommandId::new("plugin:scaffold");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginDependency {
    id: PluginId,
    version_req: VersionReq,
}

impl PluginDependency {
    pub fn new<I>(id: I, version_req: VersionReq) -> Self
    where
        I: Into<PluginId>,
    {
        Self {
            id: id.into(),
            version_req,
        }
    }

    pub fn id(&self) -> &PluginId {
        &self.id
    }

    pub fn version_req(&self) -> &VersionReq {
        &self.version_req
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginAssetKind {
    Config,
    Migration,
    Static,
}

#[derive(Clone, Debug)]
pub struct PluginAsset {
    id: PluginAssetId,
    kind: PluginAssetKind,
    target_path: PathBuf,
    contents: Arc<[u8]>,
}

impl PluginAsset {
    pub fn text<I, P>(
        id: I,
        kind: PluginAssetKind,
        target_path: P,
        contents: impl Into<String>,
    ) -> Self
    where
        I: Into<PluginAssetId>,
        P: Into<PathBuf>,
    {
        let contents = contents.into().into_bytes().into_boxed_slice();
        Self {
            id: id.into(),
            kind,
            target_path: target_path.into(),
            contents: Arc::from(contents),
        }
    }

    pub fn binary<I, P>(
        id: I,
        kind: PluginAssetKind,
        target_path: P,
        contents: impl Into<Vec<u8>>,
    ) -> Self
    where
        I: Into<PluginAssetId>,
        P: Into<PathBuf>,
    {
        let contents = contents.into().into_boxed_slice();
        Self {
            id: id.into(),
            kind,
            target_path: target_path.into(),
            contents: Arc::from(contents),
        }
    }

    pub fn id(&self) -> &PluginAssetId {
        &self.id
    }

    pub fn kind(&self) -> &PluginAssetKind {
        &self.kind
    }

    pub fn target_path(&self) -> &Path {
        &self.target_path
    }

    fn contents(&self) -> &[u8] {
        &self.contents
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginScaffoldVar {
    name: String,
    description: Option<String>,
    default: Option<String>,
}

impl PluginScaffoldVar {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            default: None,
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn default(mut self, value: impl Into<String>) -> Self {
        self.default = Some(value.into());
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description_text(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn default_value(&self) -> Option<&str> {
        self.default.as_deref()
    }
}

#[derive(Clone, Debug)]
struct PluginScaffoldFile {
    path: PathBuf,
    contents: Arc<str>,
}

#[derive(Clone, Debug)]
pub struct PluginScaffold {
    id: PluginScaffoldId,
    description: Option<String>,
    vars: Vec<PluginScaffoldVar>,
    files: Vec<PluginScaffoldFile>,
}

impl PluginScaffold {
    pub fn new<I>(id: I) -> Self
    where
        I: Into<PluginScaffoldId>,
    {
        Self {
            id: id.into(),
            description: None,
            vars: Vec::new(),
            files: Vec::new(),
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn variable(mut self, variable: PluginScaffoldVar) -> Self {
        self.vars.push(variable);
        self
    }

    pub fn file(mut self, path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        self.files.push(PluginScaffoldFile {
            path: path.into(),
            contents: Arc::from(contents.into()),
        });
        self
    }

    pub fn id(&self) -> &PluginScaffoldId {
        &self.id
    }

    pub fn description_text(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn variables(&self) -> &[PluginScaffoldVar] {
        &self.vars
    }

    pub fn files(&self) -> Vec<PathBuf> {
        self.files.iter().map(|file| file.path.clone()).collect()
    }

    fn validate(&self) -> Result<()> {
        let mut vars = BTreeSet::new();
        for variable in &self.vars {
            if !vars.insert(variable.name.clone()) {
                return Err(Error::message(format!(
                    "plugin scaffold `{}` has duplicate variable `{}`",
                    self.id, variable.name
                )));
            }
        }

        let mut files = BTreeSet::new();
        for file in &self.files {
            if !files.insert(file.path.clone()) {
                return Err(Error::message(format!(
                    "plugin scaffold `{}` has duplicate file `{}`",
                    self.id,
                    file.path.display()
                )));
            }
        }

        Ok(())
    }

    fn render(&self, values: &BTreeMap<String, String>) -> Result<Vec<(PathBuf, Vec<u8>)>> {
        let mut resolved = BTreeMap::new();
        for variable in &self.vars {
            match values.get(variable.name()) {
                Some(value) => {
                    resolved.insert(variable.name().to_string(), value.clone());
                }
                None => match variable.default_value() {
                    Some(value) => {
                        resolved.insert(variable.name().to_string(), value.to_string());
                    }
                    None => {
                        return Err(Error::message(format!(
                            "missing scaffold variable `{}` for `{}`",
                            variable.name(),
                            self.id
                        )));
                    }
                },
            }
        }

        for key in values.keys() {
            if !self.vars.iter().any(|variable| variable.name() == key) {
                return Err(Error::message(format!(
                    "unknown scaffold variable `{key}` for `{}`",
                    self.id
                )));
            }
        }

        self.files
            .iter()
            .map(|file| {
                let rendered_path = render_template(&file.path.to_string_lossy(), &resolved)?;
                Ok((
                    PathBuf::from(rendered_path),
                    render_template(&file.contents, &resolved)?.into_bytes(),
                ))
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct PluginManifest {
    id: PluginId,
    version: Version,
    forge_version: VersionReq,
    dependencies: Vec<PluginDependency>,
    description: Option<String>,
    assets: Vec<PluginAsset>,
    scaffolds: Vec<PluginScaffold>,
}

impl PluginManifest {
    pub fn new<I>(id: I, version: Version, forge_version: VersionReq) -> Self
    where
        I: Into<PluginId>,
    {
        Self {
            id: id.into(),
            version,
            forge_version,
            dependencies: Vec::new(),
            description: None,
            assets: Vec::new(),
            scaffolds: Vec::new(),
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn dependency(mut self, dependency: PluginDependency) -> Self {
        self.dependencies.push(dependency);
        self
    }

    pub fn depends_on<I>(self, id: I, version_req: VersionReq) -> Self
    where
        I: Into<PluginId>,
    {
        self.dependency(PluginDependency::new(id, version_req))
    }

    pub fn id(&self) -> &PluginId {
        &self.id
    }

    pub fn version(&self) -> &Version {
        &self.version
    }

    pub fn forge_version(&self) -> &VersionReq {
        &self.forge_version
    }

    pub fn dependencies(&self) -> &[PluginDependency] {
        &self.dependencies
    }

    pub fn description_text(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn assets(&self) -> &[PluginAsset] {
        &self.assets
    }

    pub fn scaffolds(&self) -> &[PluginScaffold] {
        &self.scaffolds
    }

    fn with_assets_and_scaffolds(
        mut self,
        assets: Vec<PluginAsset>,
        scaffolds: Vec<PluginScaffold>,
    ) -> Self {
        self.assets = assets;
        self.scaffolds = scaffolds;
        self
    }
}

#[async_trait]
pub trait Plugin: Send + Sync + 'static {
    fn manifest(&self) -> PluginManifest;

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<()>;

    async fn boot(&self, _app: &AppContext) -> Result<()> {
        Ok(())
    }
}

pub struct PluginRegistrar {
    providers: Vec<Arc<dyn ServiceProvider>>,
    routes: Vec<RouteRegistrar>,
    commands: Vec<CommandRegistrar>,
    schedules: Vec<ScheduleRegistrar>,
    websocket_routes: Vec<WebSocketRouteRegistrar>,
    validation_rules: Vec<(ValidationRuleId, Arc<dyn ValidationRule>)>,
    config_defaults: Vec<Value>,
    assets: Vec<PluginAsset>,
    scaffolds: Vec<PluginScaffold>,
}

impl Default for PluginRegistrar {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRegistrar {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            routes: Vec::new(),
            commands: Vec::new(),
            schedules: Vec::new(),
            websocket_routes: Vec::new(),
            validation_rules: Vec::new(),
            config_defaults: Vec::new(),
            assets: Vec::new(),
            scaffolds: Vec::new(),
        }
    }

    pub fn register_provider<P>(&mut self, provider: P) -> &mut Self
    where
        P: ServiceProvider,
    {
        self.providers.push(Arc::new(provider));
        self
    }

    pub fn register_routes<F>(&mut self, registrar: F) -> &mut Self
    where
        F: Fn(&mut crate::http::HttpRegistrar) -> Result<()> + Send + Sync + 'static,
    {
        self.routes.push(Arc::new(registrar));
        self
    }

    pub fn register_commands<F>(&mut self, registrar: F) -> &mut Self
    where
        F: Fn(&mut crate::cli::CommandRegistry) -> Result<()> + Send + Sync + 'static,
    {
        self.commands.push(Arc::new(registrar));
        self
    }

    pub fn register_schedule<F>(&mut self, registrar: F) -> &mut Self
    where
        F: Fn(&mut crate::scheduler::ScheduleRegistry) -> Result<()> + Send + Sync + 'static,
    {
        self.schedules.push(Arc::new(registrar));
        self
    }

    pub fn register_websocket_routes<F>(&mut self, registrar: F) -> &mut Self
    where
        F: Fn(&mut crate::websocket::WebSocketRegistrar) -> Result<()> + Send + Sync + 'static,
    {
        self.websocket_routes.push(Arc::new(registrar));
        self
    }

    pub fn register_validation_rule<I, R>(&mut self, id: I, rule: R) -> &mut Self
    where
        I: Into<ValidationRuleId>,
        R: ValidationRule,
    {
        self.validation_rules.push((id.into(), Arc::new(rule)));
        self
    }

    pub fn config_defaults(&mut self, defaults: Value) -> &mut Self {
        self.config_defaults.push(defaults);
        self
    }

    pub fn register_assets<I>(&mut self, assets: I) -> Result<&mut Self>
    where
        I: IntoIterator<Item = PluginAsset>,
    {
        let mut ids = self
            .assets
            .iter()
            .map(|asset| asset.id.clone())
            .collect::<BTreeSet<_>>();
        for asset in assets {
            if !ids.insert(asset.id.clone()) {
                return Err(Error::message(format!(
                    "plugin asset `{}` already registered",
                    asset.id
                )));
            }
            self.assets.push(asset);
        }
        Ok(self)
    }

    pub fn register_scaffolds<I>(&mut self, scaffolds: I) -> Result<&mut Self>
    where
        I: IntoIterator<Item = PluginScaffold>,
    {
        let mut ids = self
            .scaffolds
            .iter()
            .map(|scaffold| scaffold.id.clone())
            .collect::<BTreeSet<_>>();
        for scaffold in scaffolds {
            scaffold.validate()?;
            if !ids.insert(scaffold.id.clone()) {
                return Err(Error::message(format!(
                    "plugin scaffold `{}` already registered",
                    scaffold.id
                )));
            }
            self.scaffolds.push(scaffold);
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Default)]
pub struct PluginRegistry {
    plugins: Vec<PluginManifest>,
}

impl PluginRegistry {
    pub fn new(plugins: Vec<PluginManifest>) -> Self {
        Self { plugins }
    }

    pub fn plugins(&self) -> &[PluginManifest] {
        &self.plugins
    }

    pub fn plugin(&self, id: &PluginId) -> Option<&PluginManifest> {
        self.plugins.iter().find(|plugin| plugin.id() == id)
    }

    pub fn install_assets(&self, options: &PluginInstallOptions) -> Result<Vec<PathBuf>> {
        let plugins = self.select_plugins(options.plugin.as_ref(), options.all)?;
        let mut written = Vec::new();
        for plugin in plugins {
            for asset in plugin.assets() {
                let path = options.target_dir.join(asset.target_path());
                write_output_file(&path, asset.contents(), options.force)?;
                written.push(path);
            }
        }
        Ok(written)
    }

    pub fn render_scaffold(&self, options: &PluginScaffoldOptions) -> Result<Vec<PathBuf>> {
        let plugin = self.plugin(&options.plugin).ok_or_else(|| {
            Error::message(format!("plugin `{}` is not registered", options.plugin))
        })?;
        let scaffold = plugin
            .scaffolds()
            .iter()
            .find(|scaffold| scaffold.id() == &options.scaffold)
            .ok_or_else(|| {
                Error::message(format!(
                    "plugin `{}` does not expose scaffold `{}`",
                    plugin.id(),
                    options.scaffold
                ))
            })?;

        let rendered = scaffold.render(&options.values)?;
        let mut written = Vec::new();
        for (relative_path, contents) in rendered {
            let path = options.target_dir.join(relative_path);
            write_output_file(&path, &contents, options.force)?;
            written.push(path);
        }
        Ok(written)
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    fn select_plugins(&self, plugin: Option<&PluginId>, all: bool) -> Result<Vec<&PluginManifest>> {
        if all {
            return Ok(self.plugins.iter().collect());
        }

        match plugin {
            Some(plugin_id) => {
                let plugin = self.plugin(plugin_id).ok_or_else(|| {
                    Error::message(format!("plugin `{plugin_id}` is not registered"))
                })?;
                Ok(vec![plugin])
            }
            None => Err(Error::message(
                "select a plugin with `--plugin` or install from all plugins with `--all`",
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PluginInstallOptions {
    plugin: Option<PluginId>,
    all: bool,
    force: bool,
    target_dir: PathBuf,
}

impl Default for PluginInstallOptions {
    fn default() -> Self {
        Self {
            plugin: None,
            all: false,
            force: false,
            target_dir: default_target_dir(),
        }
    }
}

impl PluginInstallOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn plugin<I>(mut self, plugin: I) -> Self
    where
        I: Into<PluginId>,
    {
        self.plugin = Some(plugin.into());
        self.all = false;
        self
    }

    pub fn all(mut self) -> Self {
        self.all = true;
        self.plugin = None;
        self
    }

    pub fn force(mut self) -> Self {
        self.force = true;
        self
    }

    pub fn target_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.target_dir = path.into();
        self
    }
}

#[derive(Clone, Debug)]
pub struct PluginScaffoldOptions {
    plugin: PluginId,
    scaffold: PluginScaffoldId,
    values: BTreeMap<String, String>,
    force: bool,
    target_dir: PathBuf,
}

impl PluginScaffoldOptions {
    pub fn new<P, S>(plugin: P, scaffold: S) -> Self
    where
        P: Into<PluginId>,
        S: Into<PluginScaffoldId>,
    {
        Self {
            plugin: plugin.into(),
            scaffold: scaffold.into(),
            values: BTreeMap::new(),
            force: false,
            target_dir: default_target_dir(),
        }
    }

    pub fn set_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.values.insert(key.into(), value.into());
        self
    }

    pub fn force(mut self) -> Self {
        self.force = true;
        self
    }

    pub fn target_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.target_dir = path.into();
        self
    }
}

pub(crate) struct PreparedPlugins {
    pub(crate) registry: Arc<PluginRegistry>,
    pub(crate) instances: Vec<Arc<dyn Plugin>>,
    pub(crate) providers: Vec<Arc<dyn ServiceProvider>>,
    pub(crate) routes: Vec<RouteRegistrar>,
    pub(crate) commands: Vec<CommandRegistrar>,
    pub(crate) schedules: Vec<ScheduleRegistrar>,
    pub(crate) websocket_routes: Vec<WebSocketRouteRegistrar>,
    pub(crate) validation_rules: Vec<(ValidationRuleId, Arc<dyn ValidationRule>)>,
    pub(crate) config_defaults: Vec<Value>,
}

struct ResolvedPlugin {
    instance: Arc<dyn Plugin>,
    manifest: PluginManifest,
    providers: Vec<Arc<dyn ServiceProvider>>,
    routes: Vec<RouteRegistrar>,
    commands: Vec<CommandRegistrar>,
    schedules: Vec<ScheduleRegistrar>,
    websocket_routes: Vec<WebSocketRouteRegistrar>,
    validation_rules: Vec<(ValidationRuleId, Arc<dyn ValidationRule>)>,
    config_defaults: Vec<Value>,
}

pub(crate) fn prepare_plugins(plugins: &[Arc<dyn Plugin>]) -> Result<PreparedPlugins> {
    let ordered = resolve_plugin_order(plugins)?;
    let mut manifests = Vec::with_capacity(ordered.len());
    let mut instances = Vec::with_capacity(ordered.len());
    let mut providers = Vec::new();
    let mut routes = Vec::new();
    let mut commands = Vec::new();
    let mut schedules = Vec::new();
    let mut websocket_routes = Vec::new();
    let mut validation_rules = Vec::new();
    let mut config_defaults = Vec::new();

    for resolved in ordered {
        manifests.push(resolved.manifest);
        instances.push(resolved.instance);
        providers.extend(resolved.providers);
        routes.extend(resolved.routes);
        commands.extend(resolved.commands);
        schedules.extend(resolved.schedules);
        websocket_routes.extend(resolved.websocket_routes);
        validation_rules.extend(resolved.validation_rules);
        config_defaults.extend(resolved.config_defaults);
    }

    Ok(PreparedPlugins {
        registry: Arc::new(PluginRegistry::new(manifests)),
        instances,
        providers,
        routes,
        commands,
        schedules,
        websocket_routes,
        validation_rules,
        config_defaults,
    })
}

fn resolve_plugin_order(plugins: &[Arc<dyn Plugin>]) -> Result<Vec<ResolvedPlugin>> {
    let forge_version = Version::parse(env!("CARGO_PKG_VERSION")).map_err(Error::other)?;
    let mut nodes = Vec::with_capacity(plugins.len());
    let mut by_id = HashMap::new();

    for (index, plugin) in plugins.iter().enumerate() {
        let manifest = plugin.manifest();
        if !manifest.forge_version().matches(&forge_version) {
            return Err(Error::message(format!(
                "plugin `{}` requires Forge `{}` but this build is `{forge_version}`",
                manifest.id(),
                manifest.forge_version()
            )));
        }

        if by_id.insert(manifest.id().clone(), index).is_some() {
            return Err(Error::message(format!(
                "plugin `{}` already registered",
                manifest.id()
            )));
        }

        nodes.push((plugin.clone(), manifest));
    }

    for (_, manifest) in &nodes {
        for dependency in manifest.dependencies() {
            let dependency_manifest = nodes
                .get(*by_id.get(dependency.id()).ok_or_else(|| {
                    Error::message(format!(
                        "plugin `{}` depends on missing plugin `{}`",
                        manifest.id(),
                        dependency.id()
                    ))
                })?)
                .map(|(_, manifest)| manifest)
                .expect("plugin dependency index should exist");

            if !dependency
                .version_req()
                .matches(dependency_manifest.version())
            {
                return Err(Error::message(format!(
                    "plugin `{}` requires `{}` {} but found {}",
                    manifest.id(),
                    dependency.id(),
                    dependency.version_req(),
                    dependency_manifest.version()
                )));
            }
        }
    }

    let mut ordered_indexes = Vec::new();
    let mut permanent = BTreeSet::new();
    let mut visiting = BTreeSet::new();
    for (index, (_, manifest)) in nodes.iter().enumerate() {
        visit_plugin(
            manifest.id(),
            index,
            &nodes,
            &by_id,
            &mut permanent,
            &mut visiting,
            &mut ordered_indexes,
        )?;
    }

    let mut resolved = Vec::with_capacity(ordered_indexes.len());
    for index in ordered_indexes {
        let (instance, manifest) = nodes[index].clone();
        let mut registrar = PluginRegistrar::new();
        instance.register(&mut registrar)?;
        let manifest = manifest.with_assets_and_scaffolds(registrar.assets, registrar.scaffolds);
        resolved.push(ResolvedPlugin {
            instance,
            manifest,
            providers: registrar.providers,
            routes: registrar.routes,
            commands: registrar.commands,
            schedules: registrar.schedules,
            websocket_routes: registrar.websocket_routes,
            validation_rules: registrar.validation_rules,
            config_defaults: registrar.config_defaults,
        });
    }

    Ok(resolved)
}

fn visit_plugin(
    id: &PluginId,
    index: usize,
    nodes: &[(Arc<dyn Plugin>, PluginManifest)],
    by_id: &HashMap<PluginId, usize>,
    permanent: &mut BTreeSet<PluginId>,
    visiting: &mut BTreeSet<PluginId>,
    ordered_indexes: &mut Vec<usize>,
) -> Result<()> {
    if permanent.contains(id) {
        return Ok(());
    }

    if !visiting.insert(id.clone()) {
        return Err(Error::message(format!(
            "plugin dependency cycle detected at `{id}`"
        )));
    }

    let manifest = &nodes[index].1;
    for dependency in manifest.dependencies() {
        let dependency_index = by_id.get(dependency.id()).copied().ok_or_else(|| {
            Error::message(format!(
                "plugin `{}` depends on missing plugin `{}`",
                manifest.id(),
                dependency.id()
            ))
        })?;
        visit_plugin(
            dependency.id(),
            dependency_index,
            nodes,
            by_id,
            permanent,
            visiting,
            ordered_indexes,
        )?;
    }

    visiting.remove(id);
    permanent.insert(id.clone());
    ordered_indexes.push(index);
    Ok(())
}

pub(crate) fn builtin_cli_registrar() -> CommandRegistrar {
    Arc::new(|registry| {
        registry.command(
            PLUGIN_LIST_COMMAND,
            Command::new(PLUGIN_LIST_COMMAND.as_str().to_string())
                .about("List registered Forge plugins"),
            |invocation| async move { plugin_list_command(invocation).await },
        )?;
        registry.command(
            PLUGIN_INSTALL_ASSETS_COMMAND,
            Command::new(PLUGIN_INSTALL_ASSETS_COMMAND.as_str().to_string())
                .about("Install plugin assets into the current app")
                .arg(
                    Arg::new("plugin")
                        .long("plugin")
                        .value_name("PLUGIN_ID")
                        .help("Install assets from one plugin"),
                )
                .arg(
                    Arg::new("all")
                        .long("all")
                        .action(ArgAction::SetTrue)
                        .help("Install assets from every registered plugin"),
                )
                .arg(
                    Arg::new("to")
                        .long("to")
                        .value_name("PATH")
                        .help("Target directory for installed assets"),
                )
                .arg(
                    Arg::new("force")
                        .long("force")
                        .action(ArgAction::SetTrue)
                        .help("Overwrite existing files"),
                ),
            |invocation| async move { plugin_install_assets_command(invocation).await },
        )?;
        registry.command(
            PLUGIN_SCAFFOLD_COMMAND,
            Command::new(PLUGIN_SCAFFOLD_COMMAND.as_str().to_string())
                .about("Render a plugin scaffold into the current app")
                .arg(
                    Arg::new("plugin")
                        .long("plugin")
                        .required(true)
                        .value_name("PLUGIN_ID")
                        .help("Plugin that owns the scaffold"),
                )
                .arg(
                    Arg::new("template")
                        .long("template")
                        .required(true)
                        .value_name("SCAFFOLD_ID")
                        .help("Scaffold template identifier"),
                )
                .arg(
                    Arg::new("set")
                        .long("set")
                        .value_name("KEY=VALUE")
                        .action(ArgAction::Append)
                        .help("Assign a scaffold variable"),
                )
                .arg(
                    Arg::new("to")
                        .long("to")
                        .value_name("PATH")
                        .help("Target directory for rendered files"),
                )
                .arg(
                    Arg::new("force")
                        .long("force")
                        .action(ArgAction::SetTrue)
                        .help("Overwrite existing files"),
                ),
            |invocation| async move { plugin_scaffold_command(invocation).await },
        )?;
        Ok(())
    })
}

async fn plugin_list_command(invocation: CommandInvocation) -> Result<()> {
    let registry = invocation.app().plugins()?;
    for plugin in registry.plugins() {
        let dependencies = if plugin.dependencies().is_empty() {
            "none".to_string()
        } else {
            plugin
                .dependencies()
                .iter()
                .map(|dependency| format!("{} {}", dependency.id(), dependency.version_req()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "{} v{} | deps: {} | assets: {} | scaffolds: {}",
            plugin.id(),
            plugin.version(),
            dependencies,
            plugin.assets().len(),
            plugin.scaffolds().len()
        );
    }
    Ok(())
}

async fn plugin_install_assets_command(invocation: CommandInvocation) -> Result<()> {
    let matches = invocation.matches();
    let mut options = PluginInstallOptions::new();
    if let Some(path) = matches.get_one::<String>("to") {
        options = options.target_dir(path);
    }
    if matches.get_flag("force") {
        options = options.force();
    }
    if matches.get_flag("all") {
        options = options.all();
    } else if let Some(plugin) = matches.get_one::<String>("plugin") {
        options = options.plugin(PluginId::owned(plugin.clone()));
    }

    let registry = invocation.app().plugins()?;
    let written = registry.install_assets(&options)?;
    for path in written {
        println!("{}", path.display());
    }
    Ok(())
}

async fn plugin_scaffold_command(invocation: CommandInvocation) -> Result<()> {
    let matches = invocation.matches();
    let plugin = matches
        .get_one::<String>("plugin")
        .cloned()
        .ok_or_else(|| Error::message("missing `--plugin`"))?;
    let template = matches
        .get_one::<String>("template")
        .cloned()
        .ok_or_else(|| Error::message("missing `--template`"))?;
    let mut options =
        PluginScaffoldOptions::new(PluginId::owned(plugin), PluginScaffoldId::owned(template));
    if let Some(path) = matches.get_one::<String>("to") {
        options = options.target_dir(path);
    }
    if matches.get_flag("force") {
        options = options.force();
    }
    if let Some(values) = matches.get_many::<String>("set") {
        for assignment in values {
            let (key, value) = assignment.split_once('=').ok_or_else(|| {
                Error::message(format!(
                    "invalid scaffold assignment `{assignment}`, expected KEY=VALUE"
                ))
            })?;
            options = options.set_var(key, value);
        }
    }

    let registry = invocation.app().plugins()?;
    let written = registry.render_scaffold(&options)?;
    for path in written {
        println!("{}", path.display());
    }
    Ok(())
}

fn default_target_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn write_output_file(path: &Path, contents: &[u8], force: bool) -> Result<()> {
    if path.exists() && !force {
        return Err(Error::message(format!(
            "refusing to overwrite existing file `{}` without `--force`",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(Error::other)?;
    }
    fs::write(path, contents).map_err(Error::other)
}

fn render_template(template: &str, values: &BTreeMap<String, String>) -> Result<String> {
    let mut rendered = template.to_string();
    for (key, value) in values {
        rendered = rendered.replace(&format!("{{{{{key}}}}}"), value);
    }

    if let Some(start) = rendered.find("{{") {
        if let Some(end) = rendered[start + 2..].find("}}") {
            let unresolved = &rendered[start + 2..start + 2 + end];
            return Err(Error::message(format!(
                "unresolved scaffold variable `{}`",
                unresolved.trim()
            )));
        }
    }

    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use semver::{Version, VersionReq};
    use tempfile::tempdir;

    use super::{
        prepare_plugins, Plugin, PluginAsset, PluginAssetKind, PluginDependency, PluginId,
        PluginInstallOptions, PluginManifest, PluginRegistrar, PluginRegistry, PluginScaffold,
        PluginScaffoldOptions, PluginScaffoldVar,
    };
    use crate::foundation::{AppContext, Result, ServiceProvider, ServiceRegistrar};
    use crate::support::{PluginAssetId, PluginScaffoldId};

    struct EmptyPlugin {
        manifest: PluginManifest,
    }

    impl EmptyPlugin {
        fn new(manifest: PluginManifest) -> Self {
            Self { manifest }
        }
    }

    #[async_trait]
    impl Plugin for EmptyPlugin {
        fn manifest(&self) -> PluginManifest {
            self.manifest.clone()
        }

        fn register(&self, _registrar: &mut PluginRegistrar) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn rejects_duplicate_plugin_ids() {
        let manifest = PluginManifest::new(
            PluginId::new("forge.example"),
            Version::parse("1.0.0").unwrap(),
            VersionReq::parse("^0.1").unwrap(),
        );
        let plugins: Vec<Arc<dyn Plugin>> = vec![
            Arc::new(EmptyPlugin::new(manifest.clone())),
            Arc::new(EmptyPlugin::new(manifest)),
        ];

        let error = prepare_plugins(&plugins).err().unwrap();
        assert!(error.to_string().contains("already registered"));
    }

    #[test]
    fn rejects_missing_plugin_dependencies() {
        let manifest = PluginManifest::new(
            PluginId::new("forge.example"),
            Version::parse("1.0.0").unwrap(),
            VersionReq::parse("^0.1").unwrap(),
        )
        .dependency(PluginDependency::new(
            PluginId::new("forge.base"),
            VersionReq::parse("^1").unwrap(),
        ));
        let plugins: Vec<Arc<dyn Plugin>> = vec![Arc::new(EmptyPlugin::new(manifest))];

        let error = prepare_plugins(&plugins).err().unwrap();
        assert!(error.to_string().contains("missing plugin"));
    }

    #[test]
    fn rejects_dependency_cycles() {
        let first = PluginManifest::new(
            PluginId::new("forge.first"),
            Version::parse("1.0.0").unwrap(),
            VersionReq::parse("^0.1").unwrap(),
        )
        .dependency(PluginDependency::new(
            PluginId::new("forge.second"),
            VersionReq::parse("^1").unwrap(),
        ));
        let second = PluginManifest::new(
            PluginId::new("forge.second"),
            Version::parse("1.0.0").unwrap(),
            VersionReq::parse("^0.1").unwrap(),
        )
        .dependency(PluginDependency::new(
            PluginId::new("forge.first"),
            VersionReq::parse("^1").unwrap(),
        ));

        let plugins: Vec<Arc<dyn Plugin>> = vec![
            Arc::new(EmptyPlugin::new(first)),
            Arc::new(EmptyPlugin::new(second)),
        ];

        let error = prepare_plugins(&plugins).err().unwrap();
        assert!(error.to_string().contains("cycle"));
    }

    #[test]
    fn sorts_dependencies_before_dependents() {
        let base = PluginManifest::new(
            PluginId::new("forge.base"),
            Version::parse("1.2.0").unwrap(),
            VersionReq::parse("^0.1").unwrap(),
        );
        let dependent = PluginManifest::new(
            PluginId::new("forge.dependent"),
            Version::parse("1.0.0").unwrap(),
            VersionReq::parse("^0.1").unwrap(),
        )
        .dependency(PluginDependency::new(
            PluginId::new("forge.base"),
            VersionReq::parse("^1.2").unwrap(),
        ));
        let plugins: Vec<Arc<dyn Plugin>> = vec![
            Arc::new(EmptyPlugin::new(dependent)),
            Arc::new(EmptyPlugin::new(base)),
        ];

        let prepared = prepare_plugins(&plugins).unwrap();
        let ids = prepared
            .registry
            .plugins()
            .iter()
            .map(|manifest| manifest.id().clone())
            .collect::<Vec<_>>();

        assert_eq!(
            ids,
            vec![
                PluginId::new("forge.base"),
                PluginId::new("forge.dependent")
            ]
        );
    }

    #[test]
    fn rejects_incompatible_forge_version() {
        let plugins: Vec<Arc<dyn Plugin>> = vec![Arc::new(EmptyPlugin::new(PluginManifest::new(
            PluginId::new("forge.example"),
            Version::parse("1.0.0").unwrap(),
            VersionReq::parse(">=9").unwrap(),
        )))];

        let error = prepare_plugins(&plugins).err().unwrap();
        assert!(error.to_string().contains("requires Forge"));
    }

    #[test]
    fn installs_assets_and_detects_collisions() {
        let directory = tempdir().unwrap();
        let registry = PluginRegistry::new(vec![PluginManifest::new(
            PluginId::new("forge.example"),
            Version::parse("1.0.0").unwrap(),
            VersionReq::parse("^0.1").unwrap(),
        )
        .with_assets_and_scaffolds(
            vec![PluginAsset::text(
                PluginAssetId::new("config"),
                PluginAssetKind::Config,
                "config/plugin.toml",
                "enabled = true\n",
            )],
            Vec::new(),
        )]);

        let written = registry
            .install_assets(
                &PluginInstallOptions::new()
                    .plugin(PluginId::new("forge.example"))
                    .target_dir(directory.path()),
            )
            .unwrap();
        assert_eq!(written.len(), 1);

        let error = registry
            .install_assets(
                &PluginInstallOptions::new()
                    .plugin(PluginId::new("forge.example"))
                    .target_dir(directory.path()),
            )
            .err()
            .unwrap();
        assert!(error.to_string().contains("refusing to overwrite"));
    }

    #[test]
    fn renders_scaffolds_with_validation() {
        let directory = tempdir().unwrap();
        let registry = PluginRegistry::new(vec![PluginManifest::new(
            PluginId::new("forge.example"),
            Version::parse("1.0.0").unwrap(),
            VersionReq::parse("^0.1").unwrap(),
        )
        .with_assets_and_scaffolds(
            Vec::new(),
            vec![PluginScaffold::new(PluginScaffoldId::new("portal"))
                .variable(PluginScaffoldVar::new("name"))
                .file(
                    "src/app/{{name}}.rs",
                    "pub const NAME: &str = \"{{name}}\";\n",
                )],
        )]);

        registry
            .render_scaffold(
                &PluginScaffoldOptions::new(
                    PluginId::new("forge.example"),
                    PluginScaffoldId::new("portal"),
                )
                .set_var("name", "dashboard")
                .target_dir(directory.path()),
            )
            .unwrap();

        assert_eq!(
            fs::read_to_string(directory.path().join("src/app/dashboard.rs")).unwrap(),
            "pub const NAME: &str = \"dashboard\";\n"
        );

        let error = registry
            .render_scaffold(
                &PluginScaffoldOptions::new(
                    PluginId::new("forge.example"),
                    PluginScaffoldId::new("portal"),
                )
                .set_var("name", "dashboard")
                .set_var("extra", "value")
                .target_dir(directory.path())
                .force(),
            )
            .err()
            .unwrap();
        assert!(error.to_string().contains("unknown scaffold variable"));
    }

    struct ProviderPlugin {
        manifest: PluginManifest,
        order: Arc<Mutex<Vec<&'static str>>>,
    }

    struct MarkerProvider {
        order: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl ServiceProvider for MarkerProvider {
        async fn register(&self, registrar: &mut ServiceRegistrar) -> Result<()> {
            registrar.singleton(String::from("plugin"))?;
            self.order.lock().unwrap().push("provider-register");
            Ok(())
        }

        async fn boot(&self, _app: &AppContext) -> Result<()> {
            self.order.lock().unwrap().push("provider-boot");
            Ok(())
        }
    }

    #[async_trait]
    impl Plugin for ProviderPlugin {
        fn manifest(&self) -> PluginManifest {
            self.manifest.clone()
        }

        fn register(&self, registrar: &mut PluginRegistrar) -> Result<()> {
            registrar.register_provider(MarkerProvider {
                order: self.order.clone(),
            });
            Ok(())
        }

        async fn boot(&self, _app: &AppContext) -> Result<()> {
            self.order.lock().unwrap().push("plugin-boot");
            Ok(())
        }
    }

    #[test]
    fn plugin_registrar_collects_provider_contributions() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let plugins: Vec<Arc<dyn Plugin>> = vec![Arc::new(ProviderPlugin {
            manifest: PluginManifest::new(
                PluginId::new("forge.example"),
                Version::parse("1.0.0").unwrap(),
                VersionReq::parse("^0.1").unwrap(),
            ),
            order: order.clone(),
        })];

        let prepared = prepare_plugins(&plugins).unwrap();
        assert_eq!(prepared.providers.len(), 1);
        assert_eq!(prepared.instances.len(), 1);
    }
}
