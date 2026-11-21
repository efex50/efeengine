//! The editor's command line beyond `--socket`: which game it is for (`--package`, `--example`)
//! and the project directory builds and runs happen in. Pure, tested without a window.

use std::path::{Path, PathBuf};

/// What the editor was started for.
#[derive(Debug, Clone, PartialEq)]
pub struct Cli {
    /// The local socket name the game listens on and the editor connects to.
    pub socket: String,
    /// The cargo package (and binary) of the game. Without it Build and Run are disabled.
    pub package: Option<String>,
    /// The example directory, shown in the header.
    pub example: Option<String>,
    /// Where cargo runs and the game starts. Always inside the workspace root.
    pub project_dir: PathBuf,
}

/// Value of `--name value` or `--name=value`.
fn flag(args: &[String], name: &str) -> Result<Option<String>, String> {
    let long = format!("--{name}");
    let eq = format!("--{name}=");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if *a == long {
            return it.next().cloned().map(Some).ok_or(format!("{long} needs a value"));
        }
        if let Some(v) = a.strip_prefix(&eq) {
            return Ok(Some(v.to_string()));
        }
    }
    Ok(None)
}

/// The nearest directory at or above `from` whose `Cargo.toml` declares a `[workspace]`;
/// `from` itself when there is none.
pub fn workspace_root(from: &Path) -> PathBuf {
    for dir in from.ancestors() {
        if let Ok(t) = std::fs::read_to_string(dir.join("Cargo.toml"))
            && t.lines().any(|l| l.trim() == "[workspace]")
        {
            return dir.to_path_buf();
        }
    }
    from.to_path_buf()
}

fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Resolve `--project-dir` (relative to `cwd`) against the workspace root: it must exist and
/// lie inside the root. Without the flag the root itself is used.
pub fn resolve_project_dir(given: Option<&str>, cwd: &Path) -> Result<PathBuf, String> {
    let root = workspace_root(cwd);
    let root = root.canonicalize().map_err(|e| format!("workspace root {}: {e}", root.display()))?;
    let Some(given) = given else { return Ok(root) };
    let p = Path::new(given);
    let full = if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) };
    let dir = full.canonicalize().map_err(|_| format!("--project-dir {given}: no such directory"))?;
    if !dir.is_dir() {
        return Err(format!("--project-dir {given}: not a directory"));
    }
    if !dir.starts_with(&root) {
        return Err(format!("--project-dir {given}: outside the repository ({})", root.display()));
    }
    Ok(dir)
}

/// Parse `args` (without the program name). Without `--socket` a name is generated.
pub fn parse_cli(args: &[String], cwd: &Path) -> Result<Cli, String> {
    let socket = match flag(args, "socket")? {
        Some(n) if n.trim().is_empty() => return Err("--socket needs a name".into()),
        Some(n) => n,
        None => efeengine_editor_link::socket::new_name(),
    };
    let package = flag(args, "package")?;
    if let Some(p) = &package
        && !valid_name(p)
    {
        return Err(format!("bad package name `{p}`"));
    }
    let example = flag(args, "example")?;
    if let Some(e) = &example
        && !valid_name(e)
    {
        return Err(format!("bad example name `{e}`"));
    }
    let project_dir = resolve_project_dir(flag(args, "project-dir")?.as_deref(), cwd)?;
    Ok(Cli { socket, package, example, project_dir })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("efe-editor-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parses_flags_both_forms() {
        let root = temp("flags");
        std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        let c = parse_cli(&a(&["--socket", "mysock", "--package=demo", "--example", "demo_dir"]), &root).unwrap();
        assert_eq!((c.socket.as_str(), c.package.as_deref(), c.example.as_deref()), ("mysock", Some("demo"), Some("demo_dir")));
        assert_eq!(c.project_dir, root.canonicalize().unwrap());
        let none = parse_cli(&a(&[]), &root).unwrap();
        assert_eq!(none.socket, format!("efe-editor-{}", std::process::id()));
        assert_eq!(parse_cli(&a(&["--socket=x"]), &root).unwrap().socket, "x");
        assert!(parse_cli(&a(&["--socket"]), &root).is_err());
        assert!(parse_cli(&a(&["--socket", " "]), &root).is_err());
        assert_eq!((none.package, none.example), (None, None));
        assert!(parse_cli(&a(&["--package"]), &root).is_err());
        assert!(parse_cli(&a(&["--package", "a/b"]), &root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn project_dir_is_checked() {
        let root = temp("dirs");
        std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::create_dir_all(root.join("examples/x")).unwrap();
        let canon = root.canonicalize().unwrap();
        assert_eq!(resolve_project_dir(Some("examples/x"), &root).unwrap(), canon.join("examples/x"));
        let e = resolve_project_dir(Some("nope"), &root).unwrap_err();
        assert!(e.contains("no such directory"), "{e}");
        // `..` from inside the root still lands in the root.
        assert_eq!(resolve_project_dir(Some(".."), &root.join("examples")).unwrap(), canon);
        let outside = std::env::temp_dir();
        let e = resolve_project_dir(Some(outside.to_str().unwrap()), &root).unwrap_err();
        assert!(e.contains("outside the repository"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
