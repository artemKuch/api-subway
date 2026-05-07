use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{CoreError, DependencyKind};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_CONFIG_LIST_ITEMS: usize = 1_000;
const MAX_DEPENDENCY_RULES: usize = 512;
const MAX_PATTERN_LENGTH: usize = 1_024;
const MAX_NAME_LENGTH: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ApiSubwayConfig {
    pub version: u32,
    pub frameworks: Vec<String>,
    pub openapi: Vec<PathBuf>,
    pub output: OutputConfig,
    pub scan: ScanConfig,
    pub map: MapConfig,
    #[serde(rename = "dependency")]
    pub dependencies: Vec<DependencyRule>,
}

impl Default for ApiSubwayConfig {
    fn default() -> Self {
        Self {
            version: 1,
            frameworks: vec!["auto".to_owned()],
            openapi: Vec::new(),
            output: OutputConfig::default(),
            scan: ScanConfig::default(),
            map: MapConfig::default(),
            dependencies: Vec::new(),
        }
    }
}

impl ApiSubwayConfig {
    pub fn load(path: &Path) -> Result<Self, CoreError> {
        let contents = read_bounded_config(path)?;
        let config =
            toml::from_str::<Self>(&contents).map_err(|source| CoreError::ConfigParse {
                path: path.to_path_buf(),
                source,
            })?;
        if config.version != 1 {
            return Err(CoreError::UnsupportedConfigVersion(config.version));
        }
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), CoreError> {
        if self.frameworks.is_empty() || self.frameworks.len() > 8 {
            return Err(CoreError::InvalidConfig(
                "frameworks must contain between 1 and 8 entries".to_owned(),
            ));
        }
        if self.openapi.len() > 64 {
            return Err(CoreError::InvalidConfig(
                "openapi cannot contain more than 64 files".to_owned(),
            ));
        }
        if self.output.formats.is_empty() {
            return Err(CoreError::InvalidConfig(
                "output.formats must contain at least one format".to_owned(),
            ));
        }
        if self.output.formats.len() > 3
            || self
                .output
                .formats
                .iter()
                .enumerate()
                .any(|(index, format)| self.output.formats[..index].contains(format))
        {
            return Err(CoreError::InvalidConfig(
                "output.formats cannot contain duplicates".to_owned(),
            ));
        }
        if self.output.base.as_os_str().is_empty() {
            return Err(CoreError::InvalidConfig(
                "output.base cannot be empty".to_owned(),
            ));
        }
        if self.output.base.is_absolute()
            || self.output.base.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(CoreError::InvalidConfig(
                "output.base must be a root-relative path without '..' components".to_owned(),
            ));
        }
        if self.map.group_by != "path-prefix" {
            return Err(CoreError::InvalidConfig(format!(
                "map.group_by must be 'path-prefix', got '{}'",
                self.map.group_by
            )));
        }
        if !(1..=100).contains(&self.map.max_lines) {
            return Err(CoreError::InvalidConfig(
                "map.max_lines must be between 1 and 100".to_owned(),
            ));
        }
        if !(1..=100_000).contains(&self.map.min_line_stations) {
            return Err(CoreError::InvalidConfig(
                "map.min_line_stations must be between 1 and 100000".to_owned(),
            ));
        }
        if self
            .map
            .title
            .as_ref()
            .is_some_and(|title| title.chars().count() > MAX_NAME_LENGTH)
        {
            return Err(CoreError::InvalidConfig(format!(
                "map.title cannot exceed {MAX_NAME_LENGTH} characters"
            )));
        }
        validate_string_list("scan.exclude", &self.scan.exclude, MAX_CONFIG_LIST_ITEMS)?;
        validate_string_list(
            "map.include_routes",
            &self.map.include_routes,
            MAX_CONFIG_LIST_ITEMS,
        )?;
        validate_string_list(
            "map.exclude_routes",
            &self.map.exclude_routes,
            MAX_CONFIG_LIST_ITEMS,
        )?;
        if self.dependencies.len() > MAX_DEPENDENCY_RULES {
            return Err(CoreError::InvalidConfig(format!(
                "dependency cannot contain more than {MAX_DEPENDENCY_RULES} rules"
            )));
        }
        for framework in &self.frameworks {
            if !matches!(
                framework.to_ascii_lowercase().as_str(),
                "auto" | "next" | "nextjs" | "express" | "fastapi" | "fast-api"
            ) {
                return Err(CoreError::InvalidConfig(format!(
                    "unsupported framework '{framework}'"
                )));
            }
        }
        for dependency in &self.dependencies {
            if dependency.name.trim().is_empty()
                || dependency.name.chars().count() > MAX_NAME_LENGTH
            {
                return Err(CoreError::InvalidConfig(format!(
                    "dependency.name must contain between 1 and {MAX_NAME_LENGTH} characters"
                )));
            }
            if dependency.path_globs.is_empty() && dependency.packages.is_empty() {
                return Err(CoreError::InvalidConfig(format!(
                    "dependency '{}' needs path_globs or packages",
                    dependency.name
                )));
            }
            validate_string_list(
                "dependency.path_globs",
                &dependency.path_globs,
                MAX_CONFIG_LIST_ITEMS,
            )?;
            validate_string_list(
                "dependency.packages",
                &dependency.packages,
                MAX_CONFIG_LIST_ITEMS,
            )?;
        }
        Ok(())
    }
}

fn read_bounded_config(path: &Path) -> Result<String, CoreError> {
    let path_metadata = fs::metadata(path).map_err(|source| CoreError::ConfigRead {
        path: path.to_path_buf(),
        source,
    })?;
    if !path_metadata.is_file() {
        return Err(CoreError::ConfigNotFile(path.to_path_buf()));
    }
    let mut file = fs::File::open(path).map_err(|source| CoreError::ConfigRead {
        path: path.to_path_buf(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| CoreError::ConfigRead {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(CoreError::ConfigBudget(path.to_path_buf()));
    }
    let mut contents = String::new();
    file.by_ref()
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut contents)
        .map_err(|source| CoreError::ConfigRead {
            path: path.to_path_buf(),
            source,
        })?;
    if contents.len() as u64 > MAX_CONFIG_BYTES {
        return Err(CoreError::ConfigBudget(path.to_path_buf()));
    }
    Ok(contents)
}

fn validate_string_list(
    name: &str,
    values: &[String],
    maximum_items: usize,
) -> Result<(), CoreError> {
    if values.len() > maximum_items {
        return Err(CoreError::InvalidConfig(format!(
            "{name} cannot contain more than {maximum_items} entries"
        )));
    }
    if let Some(value) = values
        .iter()
        .find(|value| value.is_empty() || value.len() > MAX_PATTERN_LENGTH)
    {
        return Err(CoreError::InvalidConfig(format!(
            "{name} entries must contain between 1 and {MAX_PATTERN_LENGTH} bytes; invalid entry: {value:?}"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct OutputConfig {
    pub base: PathBuf,
    pub formats: Vec<OutputFormat>,
    pub theme: Theme,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            base: PathBuf::from("api-subway"),
            formats: vec![OutputFormat::Svg, OutputFormat::Html],
            theme: Theme::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Svg,
    Html,
    Json,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Auto,
    Paper,
    Midnight,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ScanConfig {
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct MapConfig {
    pub title: Option<String>,
    pub group_by: String,
    pub max_lines: usize,
    pub min_line_stations: usize,
    pub include_routes: Vec<String>,
    pub exclude_routes: Vec<String>,
}

impl Default for MapConfig {
    fn default() -> Self {
        Self {
            title: None,
            group_by: "path-prefix".to_owned(),
            max_lines: 12,
            min_line_stations: 2,
            include_routes: Vec::new(),
            exclude_routes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DependencyRule {
    pub name: String,
    pub kind: DependencyKind,
    #[serde(default)]
    pub path_globs: Vec<String>,
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default)]
    pub pin: bool,
}

#[cfg(test)]
mod tests {
    use std::{fs, fs::File};

    use crate::CoreError;

    use super::{ApiSubwayConfig, MAX_CONFIG_BYTES};

    #[test]
    fn rejects_unknown_keys_and_frameworks() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let unknown_path = directory.path().join("unknown.toml");
        fs::write(&unknown_path, "version = 1\ntyop = true\n").expect("write config");
        assert!(ApiSubwayConfig::load(&unknown_path).is_err());

        let framework_path = directory.path().join("framework.toml");
        fs::write(
            &framework_path,
            "version = 1\nframeworks = [\"not-a-framework\"]\n",
        )
        .expect("write config");
        assert!(ApiSubwayConfig::load(&framework_path).is_err());
    }

    #[test]
    fn accepts_complete_version_one_config() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            r#"
version = 1
frameworks = ["next", "express", "fastapi"]

[output]
base = "docs/api-subway"
formats = ["svg", "html", "json"]
theme = "midnight"

[scan]
exclude = ["vendor/**"]

[map]
group_by = "path-prefix"
max_lines = 12
min_line_stations = 2
include_routes = ["/api/**"]
exclude_routes = ["/api/internal/**"]

[[dependency]]
name = "Billing"
kind = "service"
packages = ["@acme/billing"]
pin = true
"#,
        )
        .expect("write config");

        let config = ApiSubwayConfig::load(&path).expect("valid config");
        assert_eq!(config.dependencies[0].name, "Billing");
    }

    #[test]
    fn rejects_oversized_configuration_before_reading_it() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("oversized.toml");
        File::create(&path)
            .and_then(|file| file.set_len(MAX_CONFIG_BYTES + 1))
            .expect("sparse oversized config");
        assert!(matches!(
            ApiSubwayConfig::load(&path),
            Err(CoreError::ConfigBudget(error_path)) if error_path == path
        ));
    }

    #[test]
    fn rejects_non_regular_configuration_inputs() {
        let directory = tempfile::tempdir().expect("temporary directory");
        assert!(matches!(
            ApiSubwayConfig::load(directory.path()),
            Err(CoreError::ConfigNotFile(error_path)) if error_path == directory.path()
        ));
    }

    #[test]
    fn rejects_unbounded_render_and_rule_settings() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("invalid.toml");
        fs::write(
            &path,
            r#"
version = 1
frameworks = ["express"]

[map]
max_lines = 0
"#,
        )
        .expect("write invalid config");
        assert!(matches!(
            ApiSubwayConfig::load(&path),
            Err(CoreError::InvalidConfig(message)) if message.contains("map.max_lines")
        ));
    }

    #[test]
    fn rejects_output_paths_that_escape_the_analysis_root() {
        let directory = tempfile::tempdir().expect("temporary directory");
        for value in ["../outside/map", "/tmp/map"] {
            let path = directory.path().join("invalid-output.toml");
            fs::write(
                &path,
                format!("version = 1\n[output]\nbase = {value:?}\nformats = [\"svg\"]\n"),
            )
            .expect("write invalid config");
            assert!(matches!(
                ApiSubwayConfig::load(&path),
                Err(CoreError::InvalidConfig(message)) if message.contains("output.base")
            ));
        }
    }
}
