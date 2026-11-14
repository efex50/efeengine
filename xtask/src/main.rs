//! efeengine build/run automation: `cargo xtask --help`.
//!
//! Rust port of build.py (which is kept). Run `cargo xtask` with no arguments in a
//! terminal to pick an example and run it native or on the web.

mod project;
mod serve;
mod toolchain;
mod util;
mod web;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};

use project::{Example, Project, WASM_TARGET};
use util::{cargo, dry_run, miss_line, ok_line, root, run, stdin_is_tty, step, warn};

#[derive(Parser)]
#[command(
    name = "cargo xtask",
    bin_name = "cargo xtask",
    about = "Build, run and serve efeengine examples (native SDL3 or web/wasm)",
    after_help = "Run without a command in a terminal to choose an example interactively.\n\
                  Environment: CARGO_TARGET_DIR is respected; WASM_BINDGEN selects a wasm-bindgen binary."
)]
struct Cli {
    /// Print commands instead of running them.
    #[arg(long, global = true)]
    dry_run: bool,
    /// Skip the SDL3 / cmake / C compiler checks before native builds.
    #[arg(long, global = true)]
    no_preflight: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Build and run an example.
    Run(BuildArgs),
    /// Build an example without running it.
    Build(BuildArgs),
    /// Release web build ready to publish (dist/<example>/).
    Dist {
        example: Option<String>,
    },
    /// Serve an existing dist/<example>/ folder without rebuilding.
    Serve {
        example: Option<String>,
        #[command(flatten)]
        server: ServerArgs,
    },
    /// List the examples.
    List,
    /// cargo check: whole workspace natively, engine + examples for wasm32.
    Check,
    /// Run the tests of calc, efepages, efescript and xtask.
    Test,
    /// Report missing tools with the command that installs each.
    Doctor,
    /// Point rust-analyzer at a target (.vscode/settings.json).
    Dev {
        /// linux, web, windows, or a full target triple.
        target: String,
    },
    /// Remove dist/<example>, or run `cargo clean` without an example.
    Clean {
        example: Option<String>,
    },
}

#[derive(Args)]
struct BuildArgs {
    /// Example name (directory under examples/). Asked for if omitted.
    example: Option<String>,
    /// native or web. Asked for if omitted.
    target: Option<Target>,
    /// Optimised build.
    #[arg(long)]
    release: bool,
    /// Build SDL3 from source and link statically (native; needs cmake + a C compiler).
    #[arg(long = "static")]
    static_sdl: bool,
    #[command(flatten)]
    server: ServerArgs,
}

#[derive(Args, Clone)]
struct ServerArgs {
    /// Web server port.
    #[arg(long, default_value_t = 8080)]
    port: u16,
    /// Address to listen on (0.0.0.0 makes it reachable from other devices).
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Target {
    Native,
    Web,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Build,
    Run,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    util::set_dry_run(cli.dry_run);
    match real_main(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(e.downcast_ref::<UsageError>().map_or(1, |_| 2))
        }
    }
}

/// Missing argument in a non-interactive context (exit code 2).
#[derive(Debug)]
struct UsageError(String);
impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (see `cargo xtask --help`)", self.0)
    }
}
impl std::error::Error for UsageError {}

fn real_main(cli: Cli) -> Result<()> {
    let preflight = !cli.no_preflight;
    let Some(cmd) = cli.cmd else {
        if !stdin_is_tty() {
            bail!(UsageError("no command given and stdin is not a terminal".into()));
        }
        return interactive(preflight);
    };
    match cmd {
        Cmd::Run(a) => build_or_run(Action::Run, a, preflight),
        Cmd::Build(a) => build_or_run(Action::Build, a, preflight),
        Cmd::Dist { example } => {
            let project = Project::load()?;
            let ex = pick_example(&project, example.as_deref())?;
            let out = web::build(&project, &ex, true)?;
            println!("Publish-ready folder: {}", out.display());
            Ok(())
        }
        Cmd::Serve { example, server } => {
            let project = Project::load()?;
            let ex = pick_example(&project, example.as_deref())?;
            serve::serve(&root().join("dist").join(&ex.dir), &server.host, server.port)
        }
        Cmd::List => list(),
        Cmd::Check => check(),
        Cmd::Test => {
            step("cargo test -p calc -p efepages -p efescript -p xtask");
            let mut c = cargo();
            c.args(["test", "-p", "calc", "-p", "efepages", "-p", "efescript", "-p", "xtask"]);
            run(c)
        }
        Cmd::Doctor => doctor(),
        Cmd::Dev { target } => dev(&target),
        Cmd::Clean { example } => clean(example.as_deref()),
    }
}

// ------------------------------------------------------------------ choosing --

fn choose(prompt: &str, items: &[&str]) -> Result<usize> {
    dialoguer::Select::new()
        .with_prompt(prompt)
        .items(items)
        .default(0)
        .interact()
        .context("selection cancelled")
}

fn pick_example(project: &Project, given: Option<&str>) -> Result<Example> {
    if let Some(name) = given {
        return project.get(name).cloned();
    }
    match project.examples.as_slice() {
        [] => bail!("no examples found in examples/"),
        [only] => {
            step(format!("only one example: {}", only.dir));
            Ok(only.clone())
        }
        many if stdin_is_tty() => {
            let names: Vec<&str> = many.iter().map(|e| e.dir.as_str()).collect();
            Ok(many[choose("Which example?", &names)?].clone())
        }
        many => bail!(UsageError(format!(
            "missing <example> (available: {})",
            many.iter().map(|e| e.dir.as_str()).collect::<Vec<_>>().join(" ")
        ))),
    }
}

fn pick_target(ex: &Example, given: Option<Target>) -> Result<Target> {
    if let Some(t) = given {
        return Ok(t);
    }
    if !stdin_is_tty() {
        bail!(UsageError("missing target (native|web)".into()));
    }
    Ok([Target::Native, Target::Web][choose(&format!("Run {} how?", ex.dir), &["native", "web"])?])
}

fn interactive(preflight: bool) -> Result<()> {
    let project = Project::load()?;
    let ex = pick_example(&project, None)?;
    let target = pick_target(&ex, None)?;
    let action = [Action::Run, Action::Build][choose("What to do?", &["run", "build only"])?];
    let server = ServerArgs { port: 8080, host: "127.0.0.1".into() };
    dispatch(&project, &ex, target, action, false, false, &server, preflight)
}

fn build_or_run(action: Action, mut a: BuildArgs, preflight: bool) -> Result<()> {
    let project = Project::load()?;
    // `run native` / `run web` with the example omitted.
    if a.target.is_none()
        && let Some(t) = a.example.as_deref().and_then(|s| Target::from_str(s, true).ok())
            && project.get(a.example.as_deref().unwrap_or_default()).is_err() {
                a.target = Some(t);
                a.example = None;
            }
    let ex = pick_example(&project, a.example.as_deref())?;
    let target = pick_target(&ex, a.target)?;
    dispatch(&project, &ex, target, action, a.release, a.static_sdl, &a.server, preflight)
}

#[allow(clippy::too_many_arguments)]
fn dispatch(
    project: &Project,
    ex: &Example,
    target: Target,
    action: Action,
    release: bool,
    static_sdl: bool,
    server: &ServerArgs,
    preflight: bool,
) -> Result<()> {
    match target {
        Target::Native => native(ex, action, release, static_sdl, preflight),
        Target::Web => {
            if static_sdl {
                warn("--static is ignored for web");
            }
            step(format!("web {}: {}", if action == Action::Run { "run" } else { "build" }, ex.dir));
            let out = web::build(project, ex, release)?;
            match action {
                Action::Build => {
                    println!("Built: {}", out.display());
                    Ok(())
                }
                Action::Run => serve::serve(&out, &server.host, server.port),
            }
        }
    }
}

// -------------------------------------------------------------------- native --

const STATIC_DEPS_HINT: &str = "hint: building SDL3 from source also needs its platform dev headers \
(look for 'Could NOT find' / 'CMake Error' above), e.g.
  Fedora: sudo dnf install wayland-devel libxkbcommon-devel libX11-devel libXext-devel mesa-libGL-devel alsa-lib-devel pulseaudio-libs-devel
  Debian: sudo apt install libwayland-dev libxkbcommon-dev libx11-dev libxext-dev libgl-dev libasound2-dev libpulse-dev
Or skip the source build: install SDL3 itself (dnf install SDL3-devel) and drop --static.";

fn preflight_native(static_sdl: bool) -> Result<()> {
    if static_sdl {
        let mut missing = vec![];
        if util::which("cmake").is_none() {
            missing.push("cmake");
        }
        if !toolchain::has_c_compiler() {
            missing.push("a C compiler");
        }
        if !missing.is_empty() {
            bail!(
                "--static builds SDL3 from source and needs: {} (not found). Install them, e.g.\n  \
                 Fedora: sudo dnf install gcc gcc-c++ cmake\n  Debian: sudo apt install build-essential cmake\n  \
                 Arch:   sudo pacman -S base-devel cmake\n  Windows: CMake + Visual Studio Build Tools\n\
                 or drop --static and use a system SDL3.",
                missing.join(", ")
            );
        }
        return Ok(());
    }
    if cfg!(windows) {
        if std::env::var_os("VCPKG_ROOT").is_none() {
            warn("on Windows SDL3 comes from vcpkg (set VCPKG_ROOT) or use --static; skipping the SDL3 check");
        }
        return Ok(());
    }
    if !toolchain::sdl3_found() {
        bail!(
            "SDL3 development files were not found (pkg-config sdl3, or libSDL3.so in a standard lib dir \
             or LIBRARY_PATH). Install them, e.g.\n  Fedora: sudo dnf install SDL3-devel\n  \
             Debian: sudo apt install libsdl3-dev\n  Arch:   sudo pacman -S sdl3\n\
             or build SDL3 from source: cargo xtask build <example> native --static (needs cmake + a C compiler).\n\
             Use --no-preflight to skip this check."
        );
    }
    Ok(())
}

fn native(ex: &Example, action: Action, release: bool, static_sdl: bool, preflight: bool) -> Result<()> {
    if preflight {
        preflight_native(static_sdl)?;
    }
    let mut extra: Vec<&str> = vec![];
    if release {
        extra.push("--release");
    }
    if static_sdl {
        extra.extend(["--features", "efeengine-core/static"]);
    }
    let desc = format!(
        "{}{}",
        if release { "release" } else { "debug" },
        if static_sdl { ", static SDL3" } else { "" }
    );
    if !ex.native_bin {
        step(format!("{} has no 'native' bin; building the lib", ex.package));
        let mut c = cargo();
        c.args(["build", "-p", &ex.package, "--lib"]).args(&extra);
        run(c)?;
        if action == Action::Run {
            bail!("example '{}' has no [[bin]] named 'native', nothing to run (library was built)", ex.dir);
        }
        return Ok(());
    }
    let verb = if action == Action::Run { "run" } else { "build" };
    step(format!("native {verb}: {} ({desc})", ex.package));
    let mut c = cargo();
    c.args([verb, "-p", &ex.package, "--bin", "native"]).args(&extra);
    run(c).inspect_err(|_| {
        if static_sdl {
            eprintln!("{STATIC_DEPS_HINT}");
        }
    })
}

// -------------------------------------------------------------- other commands --

fn list() -> Result<()> {
    let project = Project::load()?;
    if project.examples.is_empty() {
        println!("no examples found");
    }
    for e in &project.examples {
        println!(
            "{} (package {}, native bin: {})",
            e.dir,
            e.package,
            if e.native_bin { "yes" } else { "no" }
        );
    }
    Ok(())
}

fn check() -> Result<()> {
    let project = Project::load()?;
    step("cargo check --workspace (native)");
    let mut c = cargo();
    c.args(["check", "--workspace"]);
    run(c)?;
    let mut pkgs = vec!["-p".to_string(), "efeengine-core".to_string()];
    for e in &project.examples {
        pkgs.extend(["-p".to_string(), e.package.clone()]);
    }
    step(format!("cargo check for {WASM_TARGET} ({})", pkgs.join(" ")));
    let mut c = util::cargo_nightly();
    c.arg("check")
        .args(&pkgs)
        .args(["--lib", "--target", WASM_TARGET, "-Z", "build-std=panic_abort,std"]);
    run(c)
}

fn doctor() -> Result<()> {
    step("doctor");
    let mut bad = 0;
    let mut miss = |what: &str, fix: &str| {
        miss_line(what, fix);
        bad += 1;
    };
    match util::output(cargo().arg("--version")) {
        Ok(v) => ok_line(format!("cargo: {v}")),
        Err(_) => miss("cargo", "install rustup: https://rustup.rs"),
    }
    let rust = toolchain::rust_problems();
    for p in &rust {
        miss(&p.what, &p.fix);
    }
    if rust.is_empty() {
        ok_line("nightly toolchain with rust-src and the wasm32 target");
    }
    match toolchain::wasm_bindgen() {
        Ok(wb) => match (&wb.path, &wb.have) {
            (None, _) => miss("wasm-bindgen CLI (web builds)", &wb.hint()),
            (Some(_), Some(h)) if *h == wb.want => ok_line(format!("wasm-bindgen {h} (matches Cargo.lock)")),
            (Some(_), h) => miss(
                &format!("wasm-bindgen {} != Cargo.lock {}", h.clone().unwrap_or_default(), wb.want),
                &wb.hint(),
            ),
        },
        Err(e) => warn(format!("{e:#}")),
    }
    if util::which("cmake").is_some() {
        ok_line("cmake (for --static)");
    } else {
        miss("cmake (only needed for --static)", "dnf install cmake / apt install cmake");
    }
    if toolchain::has_c_compiler() {
        ok_line("C compiler (for --static)");
    } else {
        miss("C compiler (only needed for --static)", "dnf install gcc gcc-c++ / apt install build-essential");
    }
    if cfg!(windows) {
        if std::env::var_os("VCPKG_ROOT").is_some() {
            ok_line("VCPKG_ROOT set");
        } else {
            miss("VCPKG_ROOT (SDL3 via vcpkg)", "set VCPKG_ROOT or use --static");
        }
    } else if toolchain::sdl3_found() {
        ok_line("SDL3 (system)");
    } else {
        miss(
            "SDL3 dev files (not needed with --static)",
            "dnf install SDL3-devel / apt install libsdl3-dev / pacman -S sdl3",
        );
    }
    println!("{}", if bad == 0 { "All good." } else { "Some items are missing (see fixes above)." });
    Ok(())
}

fn dev(target: &str) -> Result<()> {
    let triple = match target {
        "linux" => "x86_64-unknown-linux-gnu",
        "web" => WASM_TARGET,
        "windows" => "x86_64-pc-windows-gnu",
        t => t,
    };
    if triple.is_empty() || !triple.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) {
        bail!("invalid target '{target}'");
    }
    let file: PathBuf = root().join(".vscode/settings.json");
    step(format!("rust-analyzer target -> {triple} ({})", file.display()));
    let json = format!(
        "{{\n    \"rust-analyzer.check.command\": \"check\",\n    \
         \"rust-analyzer.cargo.buildScripts.enable\": true,\n    \
         \"rust-analyzer.cargo.allTargets\": false,\n    \
         \"rust-analyzer.cargo.target\": \"{triple}\",\n    \
         \"rust-analyzer.server.extraEnv\": {{\n        \"RUSTUP_TOOLCHAIN\": \"nightly\"\n    }}\n}}\n"
    );
    if dry_run() {
        println!("[dry-run] write {}: rust-analyzer.cargo.target={triple}", file.display());
        return Ok(());
    }
    std::fs::create_dir_all(file.parent().unwrap_or(&root()))?;
    std::fs::write(&file, json).with_context(|| format!("writing {}", file.display()))
}

fn clean(example: Option<&str>) -> Result<()> {
    match example {
        Some(name) => {
            let project = Project::load()?;
            let ex = project.get(name)?;
            let dir = root().join("dist").join(&ex.dir);
            step(format!("removing {}", dir.display()));
            if dry_run() {
                println!("[dry-run] remove {}", dir.display());
            } else if dir.exists() {
                std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
            }
            Ok(())
        }
        None => {
            step("cargo clean");
            let mut c = cargo();
            c.arg("clean");
            run(c)
        }
    }
}
