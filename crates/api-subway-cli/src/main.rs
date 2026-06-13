use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    str::FromStr,
};

use api_subway_analyzers::{AnalyzeOptions, Framework, analyze};
use api_subway_core::{ApiSubwayConfig, DiagnosticSeverity, OutputFormat, Theme};
use api_subway_renderer::{RenderOptions, render_html, render_svg};
use clap::{Args, Parser, Subcommand, ValueEnum};
use thiserror::Error;

#[derive(Debug, Parser)]
#[command(
    name = "api-subway",
    version,
    about = "Generate a trustworthy API map in the visual language of a subway map"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Analyze source and write deterministic map artifacts.
    Generate(CommonArgs),
    /// Verify that existing artifacts match the current source without writing.
    Check(CommonArgs),
}

#[derive(Debug, Args)]
struct CommonArgs {
    /// Application root to scan.
    #[arg(default_value = ".")]
    root: PathBuf,
    /// Output basename. Extensions are added per format.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Framework adapter; repeat to combine adapters.
    #[arg(long, value_parser = parse_framework)]
    framework: Vec<Framework>,
    /// Artifact format; repeat to select multiple formats.
    #[arg(long, value_enum)]
    format: Vec<CliFormat>,
    /// Local `OpenAPI` 3.0/3.1 JSON or YAML document; repeat to merge.
    #[arg(long)]
    openapi: Vec<PathBuf>,
    /// Configuration file. Defaults to ROOT/.api-subway.toml when present.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Visual theme.
    #[arg(long, value_enum)]
    theme: Option<CliTheme>,
    /// Treat warnings and errors from static analysis as fatal.
    #[arg(long)]
    strict: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliFormat {
    Svg,
    Html,
    Json,
}

impl From<CliFormat> for OutputFormat {
    fn from(value: CliFormat) -> Self {
        match value {
            CliFormat::Svg => Self::Svg,
            CliFormat::Html => Self::Html,
            CliFormat::Json => Self::Json,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliTheme {
    Auto,
    Paper,
    Midnight,
}

impl From<CliTheme> for Theme {
    fn from(value: CliTheme) -> Self {
        match value {
            CliTheme::Auto => Self::Auto,
            CliTheme::Paper => Self::Paper,
            CliTheme::Midnight => Self::Midnight,
        }
    }
}

#[derive(Debug, Error)]
enum CliError {
    #[error("{0}")]
    Core(#[from] api_subway_core::CoreError),
    #[error("{0}")]
    Analyzer(#[from] api_subway_analyzers::AnalyzerError),
    #[error("{0}")]
    Renderer(#[from] api_subway_renderer::RenderError),
    #[error("failed to serialize JSON artifact: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to create output directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write artifact {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to replace artifact {path}: {source}")]
    Replace {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("output basename cannot be empty")]
    EmptyOutputBase,
    #[error("automatic configuration must be a regular file inside the analysis root: {0}")]
    UnsafeAutomaticConfig(PathBuf),
    #[error("configured output path traverses a symbolic link or leaves the analysis root: {0}")]
    UnsafeConfiguredOutput(PathBuf),
}

#[derive(Debug)]
struct Artifact {
    path: PathBuf,
    contents: String,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("api-subway: {}", sanitize_terminal_text(&error.to_string()));
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<u8, CliError> {
    let (args, check) = match cli.command {
        Command::Generate(args) => (args, false),
        Command::Check(args) => (args, true),
    };
    let root =
        args.root
            .canonicalize()
            .map_err(|source| api_subway_analyzers::AnalyzerError::Root {
                path: args.root.clone(),
                source,
            })?;
    let explicit_config = args.config.is_some();
    let config_path = args
        .config
        .as_ref()
        .map(|path| resolve_from(&root, path))
        .or_else(|| {
            let path = root.join(".api-subway.toml");
            fs::symlink_metadata(&path).is_ok().then_some(path)
        });
    if !explicit_config && let Some(path) = config_path.as_deref() {
        validate_automatic_config(&root, path)?;
    }
    let mut config = config_path
        .as_deref()
        .map(ApiSubwayConfig::load)
        .transpose()?
        .unwrap_or_default();

    let frameworks = if args.framework.is_empty() {
        parse_config_frameworks(&config.frameworks)
    } else {
        args.framework.clone()
    };
    if !args.format.is_empty() {
        config.output.formats = args.format.iter().copied().map(Into::into).collect();
    }
    if let Some(theme) = args.theme {
        config.output.theme = theme.into();
    }
    if let Some(out) = &args.out {
        config.output.base.clone_from(out);
    }
    if config.output.base.as_os_str().is_empty() {
        return Err(CliError::EmptyOutputBase);
    }

    let mut analyze_options = AnalyzeOptions::new(&root);
    analyze_options.frameworks = frameworks;
    analyze_options.openapi.clone_from(&args.openapi);
    analyze_options.config = config.clone();
    let map = analyze(&analyze_options)?;
    for diagnostic in &map.diagnostics {
        eprintln!(
            "{} [{}] {}{}",
            severity_name(diagnostic.severity),
            sanitize_terminal_text(&diagnostic.code),
            sanitize_terminal_text(&diagnostic.message),
            diagnostic
                .source
                .as_ref()
                .map_or_else(String::new, |source| format!(
                    " ({}:{})",
                    sanitize_terminal_text(&source.file),
                    source.line
                ))
        );
    }
    if args.strict
        && map
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity != DiagnosticSeverity::Info)
    {
        return Ok(2);
    }

    let render_options = RenderOptions {
        title: config.map.title.clone(),
        theme: config.output.theme,
        max_lines: config.map.max_lines,
        min_line_stations: config.map.min_line_stations,
    };
    let output_base = if args.out.is_some() {
        resolve_from(&root, &config.output.base)
    } else {
        resolve_configured_output(&root, &config.output.base)?
    };
    let artifacts = render_artifacts(&map, &output_base, &config.output.formats, &render_options)?;
    if check {
        let stale = artifacts
            .iter()
            .filter(|artifact| artifact_is_stale(artifact))
            .collect::<Vec<_>>();
        if stale.is_empty() {
            println!(
                "api-subway: artifacts are current ({} stations)",
                map.endpoints.len()
            );
            return Ok(0);
        }
        for artifact in stale {
            eprintln!(
                "stale: {}",
                sanitize_terminal_text(&artifact.path.to_string_lossy())
            );
        }
        return Ok(1);
    }

    for artifact in &artifacts {
        write_atomic(artifact)?;
        println!(
            "wrote {}",
            sanitize_terminal_text(&artifact.path.to_string_lossy())
        );
    }
    println!(
        "api-subway: {} stations, {} lines, {} diagnostics",
        map.endpoints.len(),
        map.dependencies.len(),
        map.diagnostics.len()
    );
    Ok(0)
}

fn render_artifacts(
    map: &api_subway_core::ApiMapV1,
    output_base: &Path,
    formats: &[OutputFormat],
    options: &RenderOptions,
) -> Result<Vec<Artifact>, CliError> {
    let mut artifacts = Vec::new();
    let mut extensions = BTreeSet::new();
    for format in formats {
        let extension = match format {
            OutputFormat::Svg => "svg",
            OutputFormat::Html => "html",
            OutputFormat::Json => "json",
        };
        if !extensions.insert(extension) {
            continue;
        }
        let mut contents = match format {
            OutputFormat::Svg => render_svg(map, options)?,
            OutputFormat::Html => render_html(map, options)?,
            OutputFormat::Json => serde_json::to_string_pretty(map)?,
        };
        if !contents.ends_with('\n') {
            contents.push('\n');
        }
        artifacts.push(Artifact {
            path: with_extension(output_base, extension),
            contents,
        });
    }
    artifacts.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(artifacts)
}

fn artifact_is_stale(artifact: &Artifact) -> bool {
    let Ok(link_metadata) = fs::symlink_metadata(&artifact.path) else {
        return true;
    };
    if !link_metadata.file_type().is_file() {
        return true;
    }
    let Ok(mut file) = fs::File::open(&artifact.path) else {
        return true;
    };
    let expected_bytes = artifact.contents.len() as u64;
    let Ok(metadata) = file.metadata() else {
        return true;
    };
    if metadata.len() != expected_bytes {
        return true;
    }
    let mut current = String::new();
    Read::by_ref(&mut file)
        .take(expected_bytes.saturating_add(1))
        .read_to_string(&mut current)
        .is_err()
        || current != artifact.contents
}

fn write_atomic(artifact: &Artifact) -> Result<(), CliError> {
    let parent = artifact.path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| CliError::CreateDirectory {
        path: parent.to_path_buf(),
        source,
    })?;
    let file_name = artifact
        .path
        .file_name()
        .map_or_else(|| OsString::from("api-subway"), OsString::from);
    let (mut file, temporary_path) = create_staging_file(parent, &file_name)?;
    if let Err(source) = file
        .write_all(artifact.contents.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(&temporary_path);
        return Err(CliError::Write {
            path: temporary_path.clone(),
            source,
        });
    }
    drop(file);
    if let Err(source) = replace_file(&temporary_path, &artifact.path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(CliError::Replace {
            path: artifact.path.clone(),
            source,
        });
    }
    sync_parent_directory(parent).map_err(|source| CliError::Replace {
        path: artifact.path.clone(),
        source,
    })?;
    Ok(())
}

fn create_staging_file(
    parent: &Path,
    file_name: &OsString,
) -> Result<(fs::File, PathBuf), CliError> {
    for attempt in 0..32_u8 {
        let mut temporary_name = file_name.clone();
        temporary_name.push(format!(".tmp-{}-{attempt}", std::process::id()));
        let temporary_path = parent.join(temporary_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((file, temporary_path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(CliError::Write {
                    path: temporary_path,
                    source,
                });
            }
        }
    }
    Err(CliError::Write {
        path: parent.join(file_name),
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not reserve an exclusive staging file",
        ),
    })
}

fn replace_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(temporary, destination)
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

fn parse_framework(value: &str) -> Result<Framework, String> {
    Framework::from_str(value)
}

fn parse_config_frameworks(values: &[String]) -> Vec<Framework> {
    let frameworks = values
        .iter()
        .filter_map(|value| Framework::from_str(value).ok())
        .collect::<Vec<_>>();
    if frameworks.is_empty() {
        vec![Framework::Auto]
    } else {
        frameworks
    }
}

fn resolve_from(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn validate_automatic_config(root: &Path, path: &Path) -> Result<(), CliError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| CliError::UnsafeAutomaticConfig(path.to_path_buf()))?;
    if !metadata.file_type().is_file() {
        return Err(CliError::UnsafeAutomaticConfig(path.to_path_buf()));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| CliError::UnsafeAutomaticConfig(path.to_path_buf()))?;
    if !canonical.starts_with(root) {
        return Err(CliError::UnsafeAutomaticConfig(path.to_path_buf()));
    }
    Ok(())
}

fn resolve_configured_output(root: &Path, base: &Path) -> Result<PathBuf, CliError> {
    if base.is_absolute()
        || base.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(CliError::UnsafeConfiguredOutput(root.join(base)));
    }
    let candidate = root.join(base);
    let parent = candidate.parent().unwrap_or(root);
    let relative_parent = parent
        .strip_prefix(root)
        .map_err(|_| CliError::UnsafeConfiguredOutput(candidate.clone()))?;
    let mut current = root.to_path_buf();
    for component in relative_parent.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(CliError::UnsafeConfiguredOutput(candidate));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Err(CliError::UnsafeConfiguredOutput(candidate)),
        }
    }
    Ok(candidate)
}

fn sanitize_terminal_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() && character != '\t' {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect()
}

fn with_extension(base: &Path, extension: &str) -> PathBuf {
    let mut path = base.as_os_str().to_os_string();
    path.push(".");
    path.push(extension);
    PathBuf::from(path)
}

fn severity_name(severity: DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Info => "info",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::{
        Artifact, artifact_is_stale, create_staging_file, resolve_configured_output,
        sanitize_terminal_text, with_extension,
    };

    #[test]
    fn appends_format_extensions_to_the_output_basename() {
        assert_eq!(
            with_extension(Path::new("docs/api-subway"), "svg"),
            Path::new("docs/api-subway.svg")
        );
    }

    #[test]
    fn stale_check_compares_exact_bounded_contents() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("map.json");
        let artifact = Artifact {
            path: path.clone(),
            contents: "current\n".to_owned(),
        };
        assert!(artifact_is_stale(&artifact));
        fs::write(&path, "current\n").expect("current artifact");
        assert!(!artifact_is_stale(&artifact));
        fs::write(&path, "different\n").expect("stale artifact");
        assert!(artifact_is_stale(&artifact));
        fs::remove_file(&path).expect("remove stale artifact");
        fs::create_dir(&path).expect("directory-shaped artifact");
        assert!(artifact_is_stale(&artifact));
    }

    #[test]
    fn configured_output_stays_inside_root() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let output = resolve_configured_output(directory.path(), Path::new("docs/api-subway"))
            .expect("safe output");
        assert_eq!(output, directory.path().join("docs/api-subway"));
        assert!(resolve_configured_output(directory.path(), Path::new("../outside")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn configured_output_rejects_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        symlink(outside.path(), root.path().join("docs")).expect("symlink");
        assert!(resolve_configured_output(root.path(), Path::new("docs/api-subway")).is_err());
    }

    #[test]
    fn terminal_text_cannot_emit_control_sequences() {
        assert_eq!(
            sanitize_terminal_text("bad\u{1b}[31m\nname"),
            "bad�[31m�name"
        );
    }

    #[cfg(unix)]
    #[test]
    fn staging_file_never_follows_an_existing_link() {
        use std::{ffi::OsString, os::unix::fs::symlink};

        let directory = tempfile::tempdir().expect("temporary directory");
        let target = directory.path().join("target");
        fs::write(&target, "preserve").expect("target");
        let collision = directory
            .path()
            .join(format!("map.svg.tmp-{}-0", std::process::id()));
        symlink(&target, collision).expect("collision link");
        let (_file, staging) = create_staging_file(directory.path(), &OsString::from("map.svg"))
            .expect("exclusive staging file");
        assert!(staging.ends_with(format!("map.svg.tmp-{}-1", std::process::id())));
        assert_eq!(
            fs::read_to_string(target).expect("unchanged target"),
            "preserve"
        );
    }
}
