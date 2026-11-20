//! script_demo mod in Rust (roadmap E7.4b example), the twin of `../c_mod/mod.c`.
//!
//! `frame` runs every frame: the first one adds the sum of squares 1..=5 (a `Vec`) to the game's
//! score, gives `inventory` a stack of "sword" (a `str` and a `&Item`) and reports the results; the
//! second reads the `inventory.given` event the first one caused. The mod also provides
//! `sum_squares`, `checksum` (takes a `str`) and `boom` (traps on 13) to other mods.
//!
//! `api.rs` is generated: `efescript api-export --rust script_demo.efapi inventory.efmod -o src/api.rs`
//! (see ../README.md; tests/wasm_mods_e2e.rs checks it is current).

#![no_std]

extern crate alloc;

mod api;

use alloc::string::String;
use alloc::vec::Vec;
use api::{Item, game, inventory};
use core::sync::atomic::{AtomicU32, Ordering};
use efe_guest::{provide, scratch};

scratch!(256);

/// Frames run so far.
static FRAMES: AtomicU32 = AtomicU32::new(0);

/// The most squares `sum_squares` collects.
const MAX_N: i32 = 256;

fn squares_sum(n: i32) -> i64 {
    if !(0..=MAX_N).contains(&n) {
        return -1;
    }
    let mut squares: Vec<i64> = Vec::new();
    for i in 1..=n as i64 {
        squares.push(i * i);
    }
    squares.iter().sum()
}

provide! {
    /// Sum of the squares 1..=n, collected in a heap array; -1 unless n is in 0..=256.
    fn sum_squares(n: i32) -> i64 {
        squares_sum(n)
    }
}

provide! {
    /// Weighted byte sum of `name`: the sum of (index + 1) * byte, over a heap copy of the text.
    fn checksum(name: str) -> i64 {
        let copy: String = String::from(name);
        copy.bytes().enumerate().map(|(i, b)| (i as i64 + 1) * b as i64).sum()
    }
}

provide! {
    /// Traps when `code` is 13, else returns `code * 2`.
    fn boom(code: i32) -> i32 {
        if code == 13 {
            panic!("unlucky");
        }
        code * 2
    }
}

/// Frame 1: score, give.
fn first() {
    let total = game::score_add(squares_sum(5));
    game::report(total);
    let mut item = Item::default();
    item.id = 7;
    item.count = 5;
    let status = inventory::give("sword", &item);
    game::report(status as i64);
    // the host wrote the new count through the reference
    game::report(unsafe { core::ptr::read_volatile(&item.count) } as i64);
}

/// Frame 2: read the event.
fn second() {
    game::report(inventory::given::count() as i64);
    if let Some(stack) = inventory::given::read(0) {
        game::report(stack.id as i64);
        game::report(stack.count as i64);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn frame(_dt: f32) {
    match FRAMES.fetch_add(1, Ordering::Relaxed) {
        0 => first(),
        1 => second(),
        _ => {}
    }
}
