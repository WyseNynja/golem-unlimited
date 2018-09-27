use gu_base::cli;
use plugins::parser;
use plugins::parser::PathPluginParser;
use plugins::parser::PluginParser;
use semver::Version;
use semver::VersionReq;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::fmt::Debug;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PluginMetadata {
    /// plugin name
    #[serde(default = "PluginMetadata::default_name")]
    name: String,
    /// plugin version
    #[serde(default = "PluginMetadata::default_version")]
    version: Version,
    /// vendor
    #[serde(default)]
    author: String,
    /// optional plugin description
    #[serde(default)]
    description: String,
    /// minimal required app version
    #[serde(default = "VersionReq::any")]
    gu_version_req: VersionReq,
    /// scripts to load on startup
    #[serde(default)]
    load: Vec<String>,
}

impl PluginMetadata {
    pub fn version(&self) -> Version {
        self.version.clone()
    }

    pub fn gu_version_req(&self) -> VersionReq {
        self.gu_version_req.clone()
    }

    pub fn name(&self) -> &str {
        self.name.as_ref()
    }

    fn default_name() -> String {
        "plugin".to_string()
    }

    fn default_version() -> Version {
        Version::new(0, 0, 1)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    #[serde(flatten)]
    metadata: PluginMetadata,
    status: PluginStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PluginStatus {
    Active,
    Installed,
    Error,
}

impl fmt::Display for PluginStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub fn format_plugins_table(plugins: Vec<PluginInfo>) {
    cli::format_table(
        row!["Name", "Version", "Status"],
        || "No plugins installed",
        plugins.iter().map(|plugin| {
            row![
                plugin.metadata.name,
                plugin.metadata.version.to_string(),
                plugin.status.to_string(),
            ]
        }),
    )
}

/// Trait for providing plugin files
pub trait PluginHandler: Debug {
    fn metadata(&self) -> Result<PluginMetadata, String>;

    fn file(&self, path: &Path) -> Result<Vec<u8>, String>;
}

#[derive(Debug)]
pub struct DirectoryHandler {
    directory: PathBuf,
}

impl DirectoryHandler {
    pub fn new(path: PathBuf) -> Self {
        Self { directory: path }
    }
}

impl PluginHandler for DirectoryHandler {
    fn metadata(&self) -> Result<PluginMetadata, String> {
        let metadata_file = File::open(self.directory.join("gu-plugin.json"))
            .map_err(|_| "Couldn't read metadata file".to_string())?;

        parser::parse_metadata(metadata_file)
    }

    fn file(&self, path: &Path) -> Result<Vec<u8>, String> {
        let mut file = File::open(self.directory.join(path))
            .map_err(|e| format!("Cannot open file: {:?}", e))?;

        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .map_err(|e| format!("Reading file failed: {:?}", e))?;
        Ok(buf)
    }
}

#[derive(Debug)]
pub struct ZipHandler {
    metadata: PluginMetadata,
    files: HashMap<PathBuf, Vec<u8>>,
}

impl ZipHandler {
    pub fn new(path: &PathBuf, gu_version: Version) -> Result<Self, String> {
        let mut parser = parser::ZipParser::<File>::from_path(path)?;

        let metadata = parser.validate_and_load_metadata(gu_version)?;
        let files = parser.load_files(metadata.name())?;

        Ok(Self { metadata, files })
    }
}

impl PluginHandler for ZipHandler {
    fn metadata(&self) -> Result<PluginMetadata, String> {
        Ok(self.metadata.clone())
    }

    fn file(&self, path: &Path) -> Result<Vec<u8>, String> {
        self.files
            .get(&PathBuf::from(path))
            .map(|data| data.clone())
            .ok_or(format!("File {:?} not found", path))
    }
}

#[derive(Debug)]
pub struct Plugin {
    handler: Box<PluginHandler>,
    status: PluginStatus,
}

impl Plugin {
    pub fn new<T: 'static + PluginHandler>(handler: T) -> Self {
        Self {
            handler: Box::new(handler),
            status: PluginStatus::Installed,
        }
    }

    pub fn activate(&mut self) {
        self.status = PluginStatus::Active;
    }

    pub fn inactivate(&mut self) {
        self.status = PluginStatus::Installed;
    }

    pub fn log_error(&mut self, _s: String) {
        self.status = PluginStatus::Error;
    }

    pub fn status(&self) -> PluginStatus {
        self.status.clone()
    }

    pub fn info(&self) -> Result<PluginInfo, String> {
        let meta = self.handler.metadata()?;

        Ok(PluginInfo {
            metadata: meta.clone(),
            status: self.status(),
        })
    }

    pub fn file(&self, path: &Path) -> Result<Vec<u8>, String> {
        match self.status() {
            PluginStatus::Active => self.handler.file(path),
            a => Err(format!("Plugin is not active (State - {})", a)),
        }
    }

    pub fn metadata(&self) -> Result<PluginMetadata, String> {
        self.handler.metadata()
    }
}
