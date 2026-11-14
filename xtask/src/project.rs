//! Workspace information: examples, package names, target dir, Cargo.lock versions.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use cargo_metadata::{MetadataCommand, TargetKind};

use crate::util::root;

pub const WASM_TARGET: &str = "wasm32-unknown-unknown";

#[derive(Debug, Clone)]
pub struct Example {
    /// Directory name under examples/.
    pub dir: String,
    /// Cargo package name.
    pub package: String,
    /// Has a `[[bin]] name = "native"` target.
    pub native_bin: bool,
}

impl Example {
    /// Rust crate name (`-` → `_`), used for the .wasm and JS file names.
    pub fn crate_name(&self) -> String {
        self.package.replace('-', "_")
    }
}

pub struct Project {
    pub examples: Vec<Example>,
    pub target_dir: PathBuf,
}

impl Project {
    pub fn load() -> Result<Self> {
        let root = root();
        let meta = MetadataCommand::new()
            .manifest_path(root.join("Cargo.toml"))
            .no_deps()
            .exec()
            .context("cargo metadata failed")?;
        let examples_dir = root.join("examples");
        let mut examples: Vec<Example> = meta
            .workspace_packages()
            .into_iter()
            .filter_map(|p| {
                let dir = p.manifest_path.parent()?;
                if dir.parent()? != examples_dir.as_path() {
                    return None;
                }
                Some(Example {
                    dir: dir.file_name()?.to_string(),
                    package: p.name.to_string(),
                    native_bin: p
                        .targets
                        .iter()
                        .any(|t| t.name == "native" && t.kind.contains(&TargetKind::Bin)),
                })
            })
            .collect();
        examples.sort_by(|a, b| a.dir.cmp(&b.dir));
        Ok(Self { examples, target_dir: meta.target_directory.into_std_path_buf() })
    }

    pub fn names(&self) -> Vec<String> {
        self.examples.iter().map(|e| e.dir.clone()).collect()
    }

    pub fn get(&self, name: &str) -> Result<&Example> {
        self.examples.iter().find(|e| e.dir == name || e.package == name).with_context(|| {
            format!(
                "unknown example '{name}' (available: {})",
                if self.examples.is_empty() { "none".into() } else { self.names().join(" ") }
            )
        })
    }
}

/// Version of a package as locked in Cargo.lock.
pub fn locked_version(package: &str) -> Result<String> {
    let path = root().join("Cargo.lock");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let lock: toml::Table = text.parse().context("parsing Cargo.lock")?;
    let pkgs = lock.get("package").and_then(|p| p.as_array());
    for p in pkgs.into_iter().flatten() {
        if p.get("name").and_then(|n| n.as_str()) == Some(package)
            && let Some(v) = p.get("version").and_then(|v| v.as_str()) {
                return Ok(v.to_string());
            }
    }
    bail!("{package} not found in Cargo.lock")
}
