//! Records the engine commit that new projects pin to: `git rev-parse HEAD` in the workspace
//! root. Without git (or outside a checkout) nothing is set and `create` falls back to the
//! `main` branch. `EFE_ENGINE_REV` in the environment overrides it.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=EFE_ENGINE_REV");
    println!("cargo:rerun-if-changed=build.rs");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    if let Some(head) = root.join(".git").exists().then(|| root.join(".git/HEAD")) {
        println!("cargo:rerun-if-changed={}", head.display());
    }
    let rev = std::env::var("EFE_ENGINE_REV").ok().filter(|s| !s.is_empty()).or_else(|| {
        let out = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&root).output().ok()?;
        let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (out.status.success() && s.len() >= 7 && s.chars().all(|c| c.is_ascii_hexdigit())).then_some(s)
    });
    if let Some(r) = rev {
        println!("cargo:rustc-env=EFE_ENGINE_REV={r}");
    }
}
