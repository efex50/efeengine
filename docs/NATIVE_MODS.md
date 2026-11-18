# Native mods and third-party libraries: design and roadmap

Status: planned. Nothing here is built. This document is the plan. Code waits until the editor roadmap (`EDITOR.md`) is finished.

Read first: `MOD_LOADING.md` (grants, policy, `load_input`) and `JIT.md` (Cranelift, AOT, cache).

## What this covers

Three kinds of code can reach a game, and each needs a different level of trust:

| Kind | Where it runs | Trust |
|---|---|---|
| Game scripts (`.efa`, `.efvm`) | in the engine process, through efevm | trusted: you wrote them |
| Mod bytecode (`.efmod`, `module.efvm`) | in the engine process, through efevm | untrusted: granted by the player |
| Native code (JIT, AOT library, third-party DLL) | native, see below | the highest risk |

This document covers native code. Its main rule: **a warning is not a security layer.** A third-party library loaded in the engine process can read and write all engine memory, so accepting a warning does not make it safe. Isolation is the protection, and the permission layer is what decides whether a call is allowed to start.

## Principles

1. **Nothing loads by default.** A game that does not call the load API loads no mods and no native code.
2. **The engine provides mechanism; the developer provides the decision.** The engine never shows a prompt. The game developer builds the UI that asks the player.
3. **Declared before loaded.** A mod lists every capability it needs in its manifest. Anything not declared is refused.
4. **Checked twice.** Capabilities are checked at load (imports must be granted, or the mod does not start) and on every host call (each call is guarded against the granted set).
5. **Consent is bound to content.** An approval is tied to the package hash and the requested capabilities. A changed file or a larger request asks again.
6. **Native code is a separate trust level.** It needs its own consent, and third-party libraries run isolated.

## Permissions model (Android-style)

Capabilities are declared in the manifest, as Android declares permissions in `AndroidManifest.xml`.

| Level | Examples | Granted |
|---|---|---|
| Normal | log, time, read the mod's own data folder | automatically when the mod is granted |
| Dangerous | native library, file access outside the mod folder, network, writing world state | only after the player agrees |
| Engine-only | internal calls | no mod can request them |

Declaration in `[permissions]` (manifest):

```toml
[permissions]
namespaces = ["engine.log", "world.read"]   # existing
budget = 200000                              # existing
memory_pages = 4                             # existing

[native]
required = false                             # mod still works without its native part
libraries = [
  { name = "fastmath", sha256 = "…", symbols = ["mul_vec", "dot"] },
]
```

Each capability is a *namespace* in the host registry, with a descriptor the engine writes (not the mod): the name, the level, and a plain sentence for the prompt. The mod's own text is shown as untrusted and never replaces the engine's wording.

## Consent flow

The engine has two calls; the game developer uses them with a UI of their choice.

```text
inspect_mod(bytes) -> ModInspection        // no side effects
    manifest, requested capabilities, native libraries with hashes,
    level of each, warnings, package hash

load_mod(bytes, grant: Grant)              // loads only if grant covers the request
    grant.hash must equal the bytes' hash
    grant.capabilities subset of the requested set
```

- The developer shows the inspection, collects the choice, and calls `load_mod` only if the player agreed. Without that call, nothing loads.
- A `Grant` is serialisable. The developer stores it where they like (save file, settings). The engine does not persist decisions.
- An update that requests new capabilities produces a new inspection, so the developer asks again. Removing a capability needs no prompt.
- `revoke_capability(mod_id, cap)` takes a permission back. The mod is reloaded without it, and calls that need it are refused from then on.

## Enforcement

### Load time

A mod's imports and events must lie inside its grant, or the mod does not start (`ModError::NotGranted`, see `MOD_LOADING.md`). Native libraries must match their declared hash, and their symbols must be on the declared list.

### Every host call

Each host function is registered with its capability. The instance holds the granted set as a bitset, and each host call tests one bit before it runs. A refused call returns an error to the script and is written to the log.

Rules:
- A host function without a capability cannot be registered. This is a registration error, so nothing is accidentally open.
- The check happens in efevm's host dispatch, not in the script, so a mod cannot skip it.
- Refusals are counted per mod, so a mod that keeps trying is visible in the editor's Log panel.

### Native calls

`native.call(library, symbol, args)` is a host function with capability `native:<library>`. On top of the bit check:
- The symbol must be on the library's declared list.
- Arguments are copied across the boundary. Guest memory pointers are not passed to native code.
- Each call has a timeout. An error or timeout ends that library's worker (see isolation).
- The unsafe code lives in one module, `efevm-native`, which is why `efevm`'s `forbid(unsafe_code)` is relaxed for that module and nowhere else.

## Isolation for third-party libraries

The permission checks do not stop native code that is already loaded, so third-party libraries need a separate process.

| Option | Use for | Cost |
|---|---|---|
| In-process `dlopen`, with consent | libraries we build ourselves | fast; a crash or memory bug takes the engine down |
| Out-of-process helper (recommended for third parties) | everything else | a child process per library; calls go over a pipe or shared memory with a fixed message schema; latency suits I/O and data calls, not per-frame math |
| WebAssembly build of the library | libraries that can be rebuilt for wasm | strongest sandbox; limited to what compiles to wasm |

Out-of-process helpers:
- The engine starts the helper with a minimal environment and no inherited file handles.
- The helper's stdout and stderr go to the log.
- If the helper crashes, the engine reports it, the library's calls return errors, and the helper can be restarted only after the player agrees again.

## Native code sources

Three sources, each with its own consent:

1. **Game-built JIT** (`JIT.md`): compiled in memory from a mod's bytecode. It is the same code as the bytecode, so it needs no separate consent, but it does run native. It is off unless the game enables it.
2. **AOT libraries** (`JIT.md`, `MOD_LOADING.md`): a `.efmod` may ship a shared library per target. The library is loaded only if its hash matches and the player consented to `native`.
3. **Third-party DLLs**: declared in the manifest with their hash. Always isolated (above) unless the developer marks the library as trusted in code, which should be rare.

## Roadmap

Nothing is started. Each phase ends with something that runs.

### N0: policy and consent (no native code)

- [ ] N0.1 **[S] Capability descriptors.** Each namespace has a level (normal, dangerous, engine-only) and a prompt sentence, written by the engine.
- [ ] N0.2 **[M] Inspect and load with grant.** `inspect_mod` and `load_mod(bytes, grant)`, with the hash binding and the subset check. Existing `set_mod_policy` remains for tests.
- [ ] N0.3 **[S] Runtime guard.** Each host function carries its capability; efevm tests the granted bitset on every call; refusals are logged and counted. A host function without a capability is a registration error.
- [ ] N0.4 **[S] Revoke.** `revoke_capability` reloads the mod without the capability.
- [ ] N0.5 **Tests.** A mod with an undeclared import does not start; a changed file fails the grant hash check; a call without its capability is refused at run time and logged; a game that never calls the load API loads no mods.
- _Done when_ a game with a prompt, written in the test as a function, can load a mod only after the test grants it, and the same mod is refused when the grant is missing one capability.

### N1: native libraries, in-process (built-in only)

- [ ] N1.1 **[S] Manifest `[native]`.** Parsing and validation of libraries, hashes and symbols.
- [ ] N1.2 **[M] `efevm-native` module.** `native.call` with the symbol allowlist, argument copying, and the single unsafe module.
- [ ] N1.3 **[S] Load check.** Hash of the library file must match; the library is loaded with `libloading` only after the grant.
- [ ] N1.4 **Tests.** A wrong hash is refused; an unlisted symbol is refused; a call without `native:<lib>` is refused.
- _Done when_ a built-in test library is called from a mod, and every refused call is logged.

### N2: out-of-process helpers (third-party)

- [ ] N2.1 **[M] Helper protocol.** Fixed message schema with size and count limits, over a pipe.
- [ ] N2.2 **[M] Supervisor.** Start, timeout, kill, restart-after-consent.
- [ ] N2.3 **[S] Helper sandbox.** Minimal environment, no inherited handles, logs to the engine log.
- [ ] N2.4 **Tests.** A helper that crashes returns errors to the mod and does not crash the engine; a helper that hangs is killed at the timeout.
- _Done when_ a third-party test helper runs, crashes on request, and the engine keeps running.

### N3: inspection UI for the editor and game developers

- [ ] N3.1 **[S] Example prompt.** An example game shows an inspection and a consent dialog, using the two calls.
- [ ] N3.2 **[S] Editor view.** The editor's Log panel shows refused calls per mod.
- _Done when_ the example game refuses a mod by default and loads it after the player agrees.

## Open questions

- **Browser and wasm:** native libraries cannot run on the web. A mod with a native part loads its bytecode there, and the native part is skipped with a warning shown to the player.
- **Signing:** hashes prove the file did not change; they do not prove who made it. Signing is a later decision.
- **Trusted libraries in code:** allowing in-process trust for a library without a prompt. Recommendation: not allowed at all in the first version.
- **Helper transport:** pipe or shared memory. Recommendation: pipe first; measure latency on real calls before deciding.
