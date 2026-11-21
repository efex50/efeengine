//! efeengine build/run automation: `cargo xtask --help`.
//!
//! Rust port of build.py (which is kept). Run `cargo xtask` with no arguments in a
//! terminal to pick an example and run it native or on the web.

mod api;
mod mods;
mod project;
mod remote;
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
                  Native builds also write <example>.efapi (the script API index) next to the binary and \
                  copy examples/<example>/scripts/*.efa beside it into scripts/ (the game compiles them at \
                  run time); `run` sets EFE_SCRIPT_DIR. `check-scripts` assembles them against the index. \n\
                  Mods: examples/<example>/mods/*/ (folders with a mod.toml) are packed by `mods` as a check; \
                  native builds copy the folders into mods/ next to the game, dist packs .efmod files. See `mods`.\n\
                  Remote builds: `cargo xtask --remote[=HOST] run <example> native` compiles on another machine \
                  over SSH and runs the result here (docs/REMOTE_BUILD.md).\n\
                  Environment: CARGO_TARGET_DIR is respected; WASM_BINDGEN selects a wasm-bindgen binary."
)]
struct Cli {
    /// Print commands instead of running them.
    #[arg(long, global = true)]
    dry_run: bool,
    /// Skip the SDL3 / cmake / C compiler checks before native builds.
    #[arg(long, global = true)]
    no_preflight: bool,
    /// Build on a remote machine over SSH and run here (native build/run/editor/check/test).
    /// Cargo runs on HOST after the tree is rsynced there; the built executables are fetched into
    /// target/<profile>/. Bare `--remote` uses $EFE_REMOTE_HOST, else EFE_REMOTE_HOST in remotes.env
    /// (git ignored; see remotes.env.example). Directory: EFE_REMOTE_DIR (default remote-builds/<repo>);
    /// jobs: EFE_REMOTE_JOBS (default: remote cores - 3). Not for web/dist/serve.
    #[arg(long, global = true, num_args = 0..=1, require_equals = true, value_name = "HOST", default_missing_value = "")]
    remote: Option<String>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Build and run an example.
    Run(BuildArgs),
    /// Build an example without running it.
    Build(BuildArgs),
    /// Run an example with the editor link on (EFE_EDITOR=1) and the editor window in front of it.
    /// They talk over a private local socket (EFE_EDITOR_SOCKET, chosen per session). Ends the game when the editor closes.
    Editor(EditorArgs),
    /// Build an example natively and write its API index (.efapi), the file scripts are checked against.
    ApiExport {
        example: Option<String>,
        /// Output file (default: target/efapi/<example>.efapi).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Export from a release build.
        #[arg(long)]
        release: bool,
        /// Build SDL3 from source and link statically.
        #[arg(long = "static")]
        static_sdl: bool,
        /// Add the script JIT (cargo feature efeengine-core/jit).
        #[arg(long)]
        jit: bool,
    },
    /// Release web build ready to publish (dist/<example>/, with <example>.efapi and mods/).
    Dist {
        example: Option<String>,
    },
    /// Pack examples/<example>/mods/*/ into .efmod packages (target/mods/<example>/).
    /// Mods are packed against the game's API index and against each other, in passes.
    Mods {
        example: Option<String>,
    },
    /// Write the API index fragment (.efapi) of a mod folder, built from its declaration without
    /// running the game: its provides, events, structs and [[meta.component]] components.
    ModApi {
        /// The mod folder (with mod.toml), usually examples/<example>/mods/<name>.
        dir: PathBuf,
        /// Output file (default: <folder name>.efapi in the current directory).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// The game's index to build against (default: exported from the example the mod is in).
        #[arg(long)]
        api: Option<PathBuf>,
    },
    /// Assemble every examples/<example>/scripts/*.efa against the game's API index (exported
    /// first) and fail listing the diagnostics. A script with a `; needs-mod: NAME` header is
    /// checked against the game index merged with the .efapi fragment of mods/NAME.
    CheckScripts {
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
    /// Add the script JIT (cargo feature efeengine-core/jit; native only).
    #[arg(long)]
    jit: bool,
    #[command(flatten)]
    server: ServerArgs,
}

#[derive(Args, Clone)]
struct EditorArgs {
    /// Example name (directory under examples/). Asked for if omitted.
    example: Option<String>,
    /// Optimised build.
    #[arg(long)]
    release: bool,
    /// Build SDL3 from source and link statically.
    #[arg(long = "static")]
    static_sdl: bool,
    /// Add the script JIT (cargo feature efeengine-core/jit; native only).
    #[arg(long)]
    jit: bool,
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
    /// Run the game with the editor link on, then the editor window in front of it.
    Editor,
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
    if let Some(host) = &cli.remote {
        remote::enable(Some(host.clone()))?;
    }
    let Some(cmd) = cli.cmd else {
        if !stdin_is_tty() {
            bail!(UsageError("no command given and stdin is not a terminal".into()));
        }
        return interactive(preflight);
    };
    match cmd {
        Cmd::Run(a) => build_or_run(Action::Run, a, preflight),
        Cmd::Build(a) => build_or_run(Action::Build, a, preflight),
        Cmd::Editor(a) => editor_cmd(a, preflight),
        Cmd::ApiExport { example, output, release, static_sdl, jit } => {
            let project = Project::load()?;
            let ex = pick_example(&project, example.as_deref())?;
            if preflight {
                preflight_native(static_sdl)?;
            }
            api::api_export(&project, &ex, output, release, static_sdl, jit)
        }
        Cmd::Dist { example } => {
            need_local("dist")?;
            let project = Project::load()?;
            let ex = pick_example(&project, example.as_deref())?;
            let out = web::build(&project, &ex, true)?;
            println!("Publish-ready folder: {}", out.display());
            Ok(())
        }
        Cmd::Mods { example } => {
            let project = Project::load()?;
            let ex = pick_example(&project, example.as_deref())?;
            mods::mods_cmd(&project, &ex, preflight)
        }
        Cmd::ModApi { dir, output, api } => {
            let project = Project::load()?;
            mods::mod_api_cmd(&project, &dir, output, api)
        }
        Cmd::CheckScripts { example } => {
            let project = Project::load()?;
            let ex = pick_example(&project, example.as_deref())?;
            if preflight {
                preflight_native(false)?;
            }
            api::check_scripts(&project, &ex)
        }
        Cmd::Serve { example, server } => {
            need_local("serve")?;
            let project = Project::load()?;
            let ex = pick_example(&project, example.as_deref())?;
            serve::serve(&root().join("dist").join(&ex.dir), &server.host, server.port)
        }
        Cmd::List => list(),
        Cmd::Check => check(),
        Cmd::Test => {
            step("cargo test -p calc -p efepages -p efescript -p xtask");
            if let Some(r) = remote::get() {
                return r.cargo(&["test", "-p", "calc", "-p", "efepages", "-p", "efescript", "-p", "xtask"].map(String::from));
            }
            let mut c = cargo();
            c.args(["test", "-p", "calc", "-p", "efepages", "-p", "efescript", "-p", "xtask"]);
            run(c)
        }
        Cmd::Doctor => doctor(),
        Cmd::Dev { target } => dev(&target),
        Cmd::Clean { example } => clean(example.as_deref()),
    }
}

/// `--remote` builds natively only.
fn need_local(what: &str) -> Result<()> {
    if remote::get().is_some() {
        bail!("--remote supports native builds only (not for {what})");
    }
    Ok(())
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
    dispatch(&project, &ex, target, action, false, false, false, &server, preflight)
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
    dispatch(&project, &ex, target, action, a.release, a.static_sdl, a.jit, &a.server, preflight)
}

#[allow(clippy::too_many_arguments)]
fn dispatch(
    project: &Project,
    ex: &Example,
    target: Target,
    action: Action,
    release: bool,
    static_sdl: bool,
    jit: bool,
    server: &ServerArgs,
    preflight: bool,
) -> Result<()> {
    match target {
        Target::Native => native(&project, ex, action, release, static_sdl, jit, preflight),
        Target::Web => {
            need_local("web")?;
            if static_sdl {
                warn("--static is ignored for web");
            }
            if jit {
                warn("--jit is ignored for web (the browser always uses the interpreter)");
            }
            step(format!("web {}: {}", if action == Action::Run { "run" } else { "build" }, ex.dir));
            let out = web::build(project, ex, release)?;
            match action {
                Action::Build => {
                    println!("Built: {}", out.display());
                    Ok(())
                }
                Action::Run => serve::serve(&out, &server.host, server.port),
                Action::Editor => bail!("the editor attaches to native builds only"),
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
    if remote::get().is_some() {
        // The compiler and SDL3 dev files live on the remote; here only the run-time library matters.
        remote::warn_if_no_local_sdl3(static_sdl);
        return Ok(());
    }
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

fn native(project: &Project, ex: &Example, action: Action, release: bool, static_sdl: bool, jit: bool, preflight: bool) -> Result<()> {
    if preflight {
        preflight_native(static_sdl)?;
    }
    let extra = api::build_flags(release, static_sdl, jit);
    let desc = format!(
        "{}{}{}",
        if release { "release" } else { "debug" },
        if static_sdl { ", static SDL3" } else { "" },
        if jit { ", jit" } else { "" }
    );
    if !ex.native_bin {
        if !mods::mod_folders(ex)?.is_empty() {
            warn(format!("{}: mods not packed (no binary to export the API index)", ex.dir));
        }
        step(format!("{} has no binary target; building the lib", ex.package));
        if let Some(r) = remote::get() {
            let mut args: Vec<String> = ["build", "-p", &ex.package, "--lib"].map(String::from).to_vec();
            args.extend(extra.iter().map(|s| s.to_string()));
            r.cargo(&args)?;
        } else {
            let mut c = cargo();
            c.args(["build", "-p", &ex.package, "--lib"]).args(&extra);
            run(c)?;
        }
        if action != Action::Build {
            bail!("example '{}' has no binary target, nothing to run (library was built)", ex.dir);
        }
        return Ok(());
    }
    step(format!("native build: {} ({desc}){}", ex.package, remote::get().map_or(String::new(), |r| format!(" on {}", r.host))));
    api::build_native(project, ex, release, static_sdl, jit).inspect_err(|_| {
        if static_sdl {
            eprintln!("{STATIC_DEPS_HINT}");
        }
    })?;
    let scripts = api::after_native_build(project, ex, release)?;
    let profile = api::profile_dir(project, release);
    mods::install_native(project, ex, &profile.join(format!("{}.efapi", ex.dir)), &profile.join("mods"))?;
    if action == Action::Editor {
        return editor_session(project, ex, release, scripts);
    }
    if action == Action::Run {
        step(format!("native run: {} ({desc})", ex.package));
        // With --remote the binary was fetched: start it directly (`cargo run` would rebuild here).
        let mut c = if remote::get().is_some() {
            std::process::Command::new(profile.join(format!("{}{}", ex.package, std::env::consts::EXE_SUFFIX)))
        } else {
            let mut c = cargo();
            c.args(["run", "-p", &ex.package, "--bin", &ex.package]).args(&extra);
            c
        };
        if let Some(dir) = scripts {
            c.env("EFE_SCRIPT_DIR", dir);
        }
        run(c)?;
    }
    Ok(())
}

/// Start the game with the editor link on, run the editor in front of it, and stop the game
/// when the editor closes. The game binary is started directly so that stopping it kills the game
/// and not a `cargo run` wrapper.
fn editor_session(project: &Project, ex: &Example, release: bool, scripts: Option<PathBuf>) -> Result<()> {
    // The editor is started by this process, so the session name uses this process id.
    let socket = format!("efe-editor-{}", std::process::id());
    let profile = api::profile_dir(project, release);
    let game = profile.join(format!("{}{}", ex.package, std::env::consts::EXE_SUFFIX));
    if let Some(r) = remote::get() {
        let mut args: Vec<String> = ["-p", "efeengine-editor", "--bin", "efe-editor"].map(String::from).to_vec();
        if release {
            args.push("--release".into());
        }
        remote::build_and_fetch(r, &project.target_dir, &args, release, &["efe-editor"])?;
    } else {
        let mut c = cargo();
        c.args(["build", "-p", "efeengine-editor", "--bin", "efe-editor"]);
        if release {
            c.arg("--release");
        }
        run(c)?;
    }
    let editor_run = || {
        // With --remote the editor binary was fetched: start it directly, no local cargo.
        if remote::get().is_some() {
            let mut c = std::process::Command::new(profile.join(format!("efe-editor{}", std::env::consts::EXE_SUFFIX)));
            c.args(["--socket", &socket]);
            return c;
        }
        let mut c = cargo();
        c.args(["run", "-p", "efeengine-editor", "--bin", "efe-editor", "--", "--socket", &socket]);
        if release {
            c.arg("--release");
        }
        c
    };
    step(format!("editor: {} on socket {socket}", ex.package));
    if dry_run() {
        println!("[dry-run] EFE_EDITOR=1 EFE_EDITOR_SOCKET={socket} {} &", game.display());
        return run(editor_run());
    }
    let mut g = std::process::Command::new(&game);
    g.current_dir(root()).env("EFE_EDITOR", "1").env("EFE_EDITOR_SOCKET", &socket);
    if let Some(dir) = scripts {
        g.env("EFE_SCRIPT_DIR", dir);
    }
    let mut child = g.spawn().with_context(|| format!("could not start {}", game.display()))?;
    let result = run(editor_run());
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// True when the example's sources start the editor link (`EditorLink::from_env`).
fn links_editor(ex: &Example) -> bool {
    let src = root().join("examples").join(&ex.dir).join("src");
    std::fs::read_dir(src).map(|rd| {
        rd.flatten().any(|e| {
            std::fs::read_to_string(e.path()).map(|t| t.contains("EditorLink")).unwrap_or(false)
        })
    }).unwrap_or(false)
}

fn editor_cmd(a: EditorArgs, preflight: bool) -> Result<()> {
    let project = Project::load()?;
    let ex = pick_example(&project, a.example.as_deref())?;
    if !ex.native_bin {
        bail!("example '{}' has no binary target, so there is no game to attach the editor to", ex.dir);
    }
    if !links_editor(&ex) {
        bail!(
            "example '{}' does not start the editor link: add EditorLink::from_env() to its engine setup (examples/script_demo does this)",
            ex.dir
        );
    }
    let server = ServerArgs { port: 8080, host: "127.0.0.1".into() };
    dispatch(&project, &ex, Target::Native, Action::Editor, a.release, a.static_sdl, a.jit, &server, preflight)
}

// -------------------------------------------------------------- other commands --

fn list() -> Result<()> {
    let project = Project::load()?;
    if project.examples.is_empty() {
        println!("no examples found");
    }
    for e in &project.examples {
        let scripts = std::fs::read_dir(root().join("examples").join(&e.dir).join("scripts"))
            .map(|d| d.filter_map(|f| f.ok()).filter(|f| f.path().extension().is_some_and(|x| x == "efa")).count())
            .unwrap_or(0);
        println!(
            "{} (package {}, native bin: {}, scripts: {scripts})",
            e.dir,
            e.package,
            if e.native_bin { "yes" } else { "no" }
        );
    }
    Ok(())
}

fn check() -> Result<()> {
    let project = Project::load()?;
    if let Some(r) = remote::get() {
        return check_remote(r, &project);
    }
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

/// `check --remote`: the native check runs there; the wasm check only if that machine has the target.
fn check_remote(r: &remote::Remote, project: &Project) -> Result<()> {
    step(format!("cargo check --workspace (native) on {}", r.host));
    r.cargo(&["check".to_string(), "--workspace".to_string()])?;
    if !r.has_target(WASM_TARGET)? {
        println!(
            "note: skipping the {WASM_TARGET} check: {} has no {WASM_TARGET} target \
             (rustup +nightly target add it there, or run `cargo xtask check` without --remote)",
            r.host
        );
        return Ok(());
    }
    let mut args: Vec<String> = ["check", "-p", "efeengine-core"].map(String::from).to_vec();
    for e in &project.examples {
        args.extend(["-p".to_string(), e.package.clone()]);
    }
    args.extend(["--lib", "--target", WASM_TARGET, "-Z", "build-std=panic_abort,std"].map(String::from));
    step(format!("cargo check for {WASM_TARGET} on {}", r.host));
    r.cargo(&args)
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
