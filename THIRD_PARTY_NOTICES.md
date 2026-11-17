# Third-party notices

efeengine is MIT-licensed (see `LICENSE`). This file lists code that is copied into the repository, or closely adapted from other projects, together with its license. Crates pulled in through `Cargo.toml` from crates.io are not listed here; their licenses ship with them.

## hecs (vendored as `crates/efecs`)

- Source: https://github.com/Ralith/hecs, tag `v0.11.2`, commit `c9d641251b6574c3ba157a9b4dca9046731b04a4`.
- License: MIT OR Apache-2.0, Copyright (c) Benjamin Saunders and hecs contributors. The license texts are kept unchanged in `crates/efecs/LICENSE-MIT` and `crates/efecs/LICENSE-APACHE`; `crates/efecs/macros` is Apache-2.0.
- Our changes are listed in `crates/efecs/FORK.md`. efeengine uses it under the MIT option.

## Bevy (adapted code, when used)

- Source: https://github.com/bevyengine/bevy
- License: MIT OR Apache-2.0, Copyright (c) Bevy contributors. efeengine uses it under the MIT option.
- Every file or block that copies or closely adapts Bevy code starts with:
  `// Adapted from Bevy (https://github.com/bevyengine/bevy), <path> at <commit>, MIT OR Apache-2.0, Copyright (c) Bevy contributors.`
- Adapted locations:
  - `crates/efecs/src/tick.rs`: the wrap-aware `Tick` comparison (`is_newer_than`), `check_tick` clamping and the `CHECK_TICK_THRESHOLD` / `MAX_CHANGE_AGE` constants, adapted from `crates/bevy_ecs/src/component.rs` and `change_detection.rs` at v0.14.0.

### Bevy MIT license text

```
MIT License

Copyright (c) 2020 Carter Anderson

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Inter (bundled font, `crates/ui/assets/`)

- Source: https://github.com/rsms/inter, release `v4.1` (`extras/ttf/Inter-Regular.ttf`, `extras/ttf/Inter-Bold.ttf`, unmodified).
- License: SIL Open Font License 1.1, Copyright (c) The Inter Project Authors. The license text is kept unchanged in `crates/ui/assets/Inter-OFL.txt`. Used as the default font of the native UI backend (`crates/ui/src/native`).
