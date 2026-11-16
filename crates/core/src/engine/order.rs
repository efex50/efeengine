//! System order numbers. Inside a stage, systems run sorted by `(order, registration index)`;
//! the default is [`USER`]. Set one with `engine.add_system(..).order(n)`.
//!
//! The gaps between the constants are wide on purpose, so plugins can slot systems between
//! them (for example `order::PHYSICS - 1`) without renumbering anything.

/// Reads input and turns it into game events; before everything else.
pub const INPUT: i32 = -1000;
/// Early game logic that other systems depend on.
pub const EARLY: i32 = -500;
/// The script system (runs the loaded script modules).
pub const SCRIPTS: i32 = -100;
/// Default order of `add_system` / `add_par_system`.
pub const USER: i32 = 0;
/// Physics (for example a physics plugin in `FixedUpdate`).
pub const PHYSICS: i32 = 500;
/// Transform propagation (`GlobalTransform` from `Transform` and `Parent`).
pub const TRANSFORM: i32 = 900;
/// Late systems that read final transforms (camera follow, culling).
pub const LATE: i32 = 1000;
