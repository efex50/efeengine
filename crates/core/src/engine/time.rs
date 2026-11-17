//! Frame clock and fixed-timestep accumulator.
//!
//! Tracks elapsed time, frame delta (clamped to prevent spiral of death on hitches),
//! and fixed-step simulation time for deterministic updates across native and WebAssembly.

/// Largest frame delta accepted, in seconds; longer frames (debugger, background tab) are clamped.
pub const MAX_DELTA: f64 = 0.25;

/// At most this many FixedUpdate steps per frame; any remaining accumulated time is dropped.
pub const MAX_FIXED_STEPS: u32 = 8;

/// Default fixed time step: 60 Hz (1 / 60 seconds).
pub const DEFAULT_FIXED_DT: f64 = 1.0 / 60.0;

/// Frame clock with a fixed-step accumulator.
///
/// Designed to work on all platforms including WebAssembly by accepting monotonic
/// timestamps from the caller (`now_secs`) rather than reading the system clock directly.
#[derive(Debug, Clone)]
pub struct Time {
    delta: f64,
    fixed_dt: f64,
    elapsed: f64,
    frame: u64,
    accumulator: f64,
    last_now: Option<f64>,
    in_fixed: bool,
}

impl Default for Time {
    fn default() -> Self {
        Self::new(DEFAULT_FIXED_DT)
    }
}

impl Time {
    /// Creates a new frame clock with the given fixed time step in seconds.
    ///
    /// If `fixed_dt <= 0.0`, is NaN, or is infinite, falls back to [`DEFAULT_FIXED_DT`].
    pub fn new(fixed_dt: f64) -> Self {
        let fixed_dt = if fixed_dt > 0.0 && fixed_dt.is_finite() {
            fixed_dt
        } else {
            DEFAULT_FIXED_DT
        };
        Self {
            delta: 0.0,
            fixed_dt,
            elapsed: 0.0,
            frame: 0,
            accumulator: 0.0,
            last_now: None,
            in_fixed: false,
        }
    }

    /// Frame delta in seconds (clamped to [`MAX_DELTA`]). Inside FixedUpdate it equals [`fixed_delta()`](Self::fixed_delta).
    pub fn delta(&self) -> f32 {
        if self.in_fixed {
            self.fixed_delta()
        } else {
            self.delta as f32
        }
    }

    /// Frame delta in seconds as `f64` (clamped to [`MAX_DELTA`]). Inside FixedUpdate it equals [`fixed_delta_f64()`](Self::fixed_delta_f64).
    pub fn delta_f64(&self) -> f64 {
        if self.in_fixed {
            self.fixed_delta_f64()
        } else {
            self.delta
        }
    }

    /// Fixed time step in seconds as `f32`.
    pub fn fixed_delta(&self) -> f32 {
        self.fixed_dt as f32
    }

    /// Fixed time step in seconds as `f64`.
    pub fn fixed_delta_f64(&self) -> f64 {
        self.fixed_dt
    }

    /// Sum of clamped deltas since the first frame.
    pub fn elapsed(&self) -> f64 {
        self.elapsed
    }

    /// Frame index: 0 during the first frame, +1 on every later [`advance()`](Self::advance).
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Accumulator / fixed_dt after this frame's fixed steps were taken, in `[0, 1)`: interpolation factor.
    pub fn alpha(&self) -> f32 {
        let a = (self.accumulator / self.fixed_dt) as f32;
        if a.is_nan() || a <= 0.0 {
            0.0
        } else if a >= 1.0 {
            1.0 - f32::EPSILON
        } else {
            a
        }
    }

    /// Remember `now_secs` as the previous frame time without advancing anything (frame
    /// count, delta, elapsed and accumulator stay). Used while the engine is paused, so the
    /// first frame after resuming sees a normal delta, not the whole paused time.
    pub fn track(&mut self, now_secs: f64) {
        if now_secs.is_finite() {
            self.last_now = Some(now_secs);
        }
    }

    /// Start a new frame at time `now_secs` (any monotonic origin). First call: delta 0.
    ///
    /// A `now` earlier than the previous one (clock reset) counts as delta 0. Clamps the delta,
    /// adds it to `elapsed` and the accumulator, then returns how many fixed steps to run this
    /// frame (at most [`MAX_FIXED_STEPS`]), subtracting `steps * fixed_dt` from the accumulator;
    /// if the cap was hit, the leftover is reduced to `accumulator % fixed_dt` so the backlog is dropped.
    /// Non-finite `now_secs` counts as delta 0.
    pub fn advance(&mut self, now_secs: f64) -> u32 {
        let delta = if let Some(last) = self.last_now {
            self.frame += 1;
            if !now_secs.is_finite() || now_secs < last {
                0.0
            } else {
                now_secs - last
            }
        } else {
            0.0
        };

        if now_secs.is_finite() {
            self.last_now = Some(now_secs);
        } else if self.last_now.is_none() {
            self.last_now = Some(0.0);
        }

        let clamped_delta = if delta > MAX_DELTA {
            MAX_DELTA
        } else if delta < 0.0 {
            0.0
        } else {
            delta
        };

        self.delta = clamped_delta;
        self.elapsed += clamped_delta;
        self.accumulator += clamped_delta;

        let mut steps = 0;
        while self.accumulator >= self.fixed_dt * (1.0 - 1e-9) && steps < MAX_FIXED_STEPS {
            self.accumulator -= self.fixed_dt;
            steps += 1;
        }

        if steps == MAX_FIXED_STEPS && self.accumulator > 0.0 {
            self.accumulator %= self.fixed_dt;
        }

        if self.accumulator < 0.0 {
            self.accumulator = 0.0;
        }

        steps
    }

    /// The engine sets this around FixedUpdate so `delta()` reports the fixed step.
    pub fn set_in_fixed(&mut self, in_fixed: bool) {
        self.in_fixed = in_fixed;
    }

    /// Sets the fixed time step in seconds.
    ///
    /// If `fixed_dt <= 0.0`, is NaN, or is infinite, falls back to [`DEFAULT_FIXED_DT`].
    pub fn set_fixed_delta(&mut self, fixed_dt: f64) {
        self.fixed_dt = if fixed_dt > 0.0 && fixed_dt.is_finite() {
            fixed_dt
        } else {
            DEFAULT_FIXED_DT
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_advance() {
        let mut time = Time::new(DEFAULT_FIXED_DT);
        let steps = time.advance(100.0);
        assert_eq!(time.delta_f64(), 0.0);
        assert_eq!(time.delta(), 0.0);
        assert_eq!(time.frame(), 0);
        assert_eq!(steps, 0);
        assert_eq!(time.elapsed(), 0.0);
    }

    #[test]
    fn test_advance_one_step() {
        let mut time = Time::new(1.0 / 60.0);
        let s0 = time.advance(0.0);
        assert_eq!(s0, 0);
        assert_eq!(time.frame(), 0);

        let s1 = time.advance(1.0 / 60.0);
        assert_eq!(s1, 1);
        assert_eq!(time.frame(), 1);
    }

    #[test]
    fn test_sixty_frames_of_sixtieth() {
        let mut time = Time::new(1.0 / 60.0);
        let mut total_steps = 0;

        // Frame 0: delta 0, 0 steps
        total_steps += time.advance(0.0);
        assert_eq!(total_steps, 0);
        assert_eq!(time.frame(), 0);

        for i in 1..=60 {
            let now = i as f64 / 60.0;
            total_steps += time.advance(now);
        }
        assert_eq!(time.frame(), 60);
        assert_eq!(total_steps, 60);
    }

    #[test]
    fn test_ten_second_hitch() {
        let mut time = Time::new(1.0 / 60.0);
        time.advance(0.0);

        let steps = time.advance(10.0);
        assert_eq!(time.delta_f64(), 0.25);
        assert_eq!(time.delta(), 0.25);
        assert_eq!(steps, 8);

        let alpha = time.alpha();
        assert!(alpha >= 0.0 && alpha < 1.0, "alpha {alpha} not in [0, 1)");
    }

    #[test]
    fn test_thirty_fps_gives_two_steps_each() {
        let mut time = Time::new(1.0 / 60.0);
        time.advance(0.0);

        for i in 1..=5 {
            let now = i as f64 / 30.0;
            let steps = time.advance(now);
            assert_eq!(steps, 2, "frame {i} should yield 2 steps");
        }
    }

    #[test]
    fn test_in_fixed_delta() {
        let mut time = Time::new(1.0 / 60.0);
        time.advance(0.0);
        time.advance(1.0 / 30.0);

        assert!((time.delta() - (1.0 / 30.0) as f32).abs() < 1e-6);
        assert_ne!(time.delta(), time.fixed_delta());

        time.set_in_fixed(true);
        assert_eq!(time.delta(), time.fixed_delta());
        assert_eq!(time.delta_f64(), time.fixed_delta_f64());

        time.set_in_fixed(false);
        assert_ne!(time.delta(), time.fixed_delta());
    }

    #[test]
    fn test_backwards_clock() {
        let mut time = Time::new(1.0 / 60.0);
        time.advance(10.0);
        let steps = time.advance(5.0);
        assert_eq!(time.delta_f64(), 0.0);
        assert_eq!(time.delta(), 0.0);
        assert_eq!(steps, 0);
    }

    #[test]
    fn test_fallback_to_default_fixed_dt() {
        let t0 = Time::new(0.0);
        assert_eq!(t0.fixed_delta_f64(), DEFAULT_FIXED_DT);
        let t_neg = Time::new(-1.0);
        assert_eq!(t_neg.fixed_delta_f64(), DEFAULT_FIXED_DT);
        let t_nan = Time::new(f64::NAN);
        assert_eq!(t_nan.fixed_delta_f64(), DEFAULT_FIXED_DT);
        let t_inf = Time::new(f64::INFINITY);
        assert_eq!(t_inf.fixed_delta_f64(), DEFAULT_FIXED_DT);

        let mut t = Time::new(1.0 / 120.0);
        t.set_fixed_delta(0.0);
        assert_eq!(t.fixed_delta_f64(), DEFAULT_FIXED_DT);
    }

    #[test]
    fn test_elapsed_sums_clamped_deltas() {
        let mut time = Time::new(1.0 / 60.0);
        assert_eq!(time.elapsed(), 0.0);

        time.advance(0.0);
        assert_eq!(time.elapsed(), 0.0);

        time.advance(0.1);
        assert!((time.elapsed() - 0.1).abs() < 1e-9);

        time.advance(10.1); // delta 10.0 is clamped to 0.25
        assert!((time.elapsed() - 0.35).abs() < 1e-9);
    }

    #[test]
    fn test_tiny_fixed_dt_zero_elapsed_gives_no_steps() {
        let mut time = Time::new(1e-10);
        assert_eq!(time.advance(0.0), 0);
        assert_eq!(time.advance(0.0), 0);
        assert_eq!(time.alpha(), 0.0);
    }

    #[test]
    fn test_non_finite_now() {
        let mut time = Time::new(1.0 / 60.0);
        let s0 = time.advance(f64::NAN);
        assert_eq!(s0, 0);
        assert_eq!(time.delta_f64(), 0.0);
        assert_eq!(time.frame(), 0);

        let s1 = time.advance(1.0 / 60.0);
        assert_eq!(s1, 1);
        assert_eq!(time.frame(), 1);

        let s2 = time.advance(f64::INFINITY);
        assert_eq!(s2, 0);
        assert_eq!(time.delta_f64(), 0.0);
    }
}
