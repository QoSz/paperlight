//! Pure kinetic-scroll math: touchpad velocity on finger lift, then GTK-like exponential decay.

/// GTK's kinetic friction, per second: speed decays as `exp(-DECEL·t)`, so a fling coasts `v0 / DECEL`.
const DECEL: f32 = 4.0;
/// A fling stops below this speed, in points per second.
const STOP_SPEED: f32 = 10.0;
/// A finger lift slower than this, in points per second, does not fling.
const MIN_FLING: f32 = 80.0;
/// Weight of the newest frame in the velocity average: about 3 frames (~50 ms) of memory.
const EMA_WEIGHT: f32 = 0.5;
/// A lift this long after the last motion, in seconds, does not fling (the finger rested).
const STALE: f64 = 0.1;

/// Seconds between two frame times, as `f32`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "frame intervals are tiny; f32 precision is plenty"
)]
fn secs(from: f64, to: f64) -> f32 {
    (to - from) as f32
}

/// Touchpad scroll velocity of one gesture, from per-event deltas stamped with frame times.
/// Events of one frame share its time, so the velocity is sampled once per frame.
#[derive(Debug, Default)]
pub struct VelocityTracker {
    /// Average velocity in points per second; `None` until the first sample.
    v: Option<f32>,
    /// Time of the last frame folded into `v`.
    last: Option<f64>,
    /// The frame being collected: its time and summed delta.
    frame: Option<(f64, f32)>,
}

impl VelocityTracker {
    /// Fingers down at frame time `time`: forgets the previous gesture. This frame has no valid
    /// interval, so it only sets the time base (its deltas are not a sample).
    pub fn start(&mut self, time: f64) {
        *self = Self {
            frame: Some((time, 0.0)),
            ..Self::default()
        };
    }

    /// One scroll event of `dy` points at frame time `time`.
    pub fn add(&mut self, dy: f32, time: f64) {
        match self.frame {
            Some((t, ref mut sum)) if t.to_bits() == time.to_bits() => *sum += dy,
            _ => {
                self.fold();
                self.frame = Some((time, dy));
            }
        }
    }

    /// Fingers up at frame time `time`: the fling velocity, if the motion was fast and recent.
    /// Resets the tracker.
    pub fn end(&mut self, time: f64) -> Option<f32> {
        self.fold();
        let (v, last) = (self.v?, self.last?);
        *self = Self::default();
        (time - last <= STALE && v.abs() >= MIN_FLING).then_some(v)
    }

    /// Folds the collected frame into the average. The first frame only sets the time base.
    fn fold(&mut self) {
        let Some((time, dy)) = self.frame.take() else {
            return;
        };
        if let Some(last) = self.last {
            let dt = secs(last, time);
            if dt > 0.0 {
                let sample = dy / dt;
                self.v = Some(
                    self.v
                        .map_or(sample, |v| EMA_WEIGHT * sample + (1.0 - EMA_WEIGHT) * v),
                );
            }
        }
        self.last = Some(time);
    }
}

/// A coasting scroll: velocity in points per second (scroll-offset direction) and the time of the
/// last step.
#[derive(Debug, Clone, Copy)]
pub struct Fling {
    v: f32,
    time: f64,
}

impl Fling {
    pub fn new(v: f32, time: f64) -> Self {
        Self { v, time }
    }

    /// Advances to frame time `time`: the distance to scroll, and the fling, or `None` once it is
    /// slower than the stop speed. Integrates the decay exactly, so it is frame-rate independent.
    pub fn step(self, time: f64) -> (f32, Option<Self>) {
        let decay = (-DECEL * secs(self.time, time).max(0.0)).exp();
        let distance = self.v * (1.0 - decay) / DECEL;
        let v = self.v * decay;
        (
            distance,
            (v.abs() >= STOP_SPEED).then_some(Self { v, time }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f64 = 1.0 / 60.0;

    /// A gesture: `start` at 0, then one `add(dy)` per frame, lifted one frame after the last.
    fn gesture(frames: &[f32]) -> Option<f32> {
        let mut v = VelocityTracker::default();
        v.start(0.0);
        let mut t = 0.0;
        for &dy in frames {
            v.add(dy, t);
            t += FRAME;
        }
        v.end(t)
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-2
    }

    #[test]
    fn ema_ignores_first_sample() {
        // The start frame (1000 pt, no valid dt) must not count; every later frame is 600 pt/s.
        let v = gesture(&[1000.0, 10.0, 10.0, 10.0]).unwrap_or_default();
        assert!(close(v, 600.0), "{v}");
    }

    #[test]
    fn ema_weights_recent_frames() {
        // Frames at 600 then 1200 pt/s: 0.5 · 1200 + 0.5 · 600.
        let v = gesture(&[0.0, 10.0, 20.0]).unwrap_or_default();
        assert!(close(v, 900.0), "{v}");
    }

    #[test]
    fn events_in_one_frame_are_summed() {
        let mut v = VelocityTracker::default();
        v.start(0.0);
        v.add(5.0, FRAME);
        v.add(5.0, FRAME);
        let got = v.end(FRAME).unwrap_or_default();
        assert!(close(got, 600.0), "{got}");
    }

    #[test]
    fn slow_paused_or_unstarted_lift_does_not_fling() {
        // 60 pt/s is under the fling threshold.
        assert_eq!(gesture(&[0.0, 1.0, 1.0]), None);
        // Finger rested 0.2 s before lifting.
        let mut v = VelocityTracker::default();
        v.start(0.0);
        v.add(10.0, FRAME);
        v.add(10.0, 2.0 * FRAME);
        assert_eq!(v.end(2.0 * FRAME + 0.2), None);
        // A lift without any motion.
        assert_eq!(VelocityTracker::default().end(1.0), None);
    }

    #[test]
    fn end_resets_tracker() {
        let mut v = VelocityTracker::default();
        v.start(0.0);
        v.add(10.0, FRAME);
        v.add(10.0, 2.0 * FRAME);
        assert!(v.end(2.0 * FRAME).is_some());
        assert_eq!(v.end(2.0 * FRAME), None);
    }

    #[test]
    fn decay_stops_within_1_6_s_from_500() {
        let mut fling = Some(Fling::new(500.0, 0.0));
        let (mut t, mut distance) = (0.0, 0.0);
        while let Some(f) = fling {
            t += FRAME;
            assert!(t < 1.6, "still flinging at {t} s");
            let (dy, next) = f.step(t);
            distance += dy;
            fling = next;
        }
        // Exact integration: (500 − 10) / DECEL travelled before the stop threshold.
        assert!((distance - 490.0 / DECEL).abs() < 1.0, "{distance}");
    }

    #[test]
    fn step_is_frame_rate_independent() {
        let one = Fling::new(-1000.0, 0.0).step(0.25).0;
        let mut f = Some(Fling::new(-1000.0, 0.0));
        let mut many = 0.0;
        for i in 1..=25 {
            let Some(cur) = f else { break };
            let (dy, next) = cur.step(f64::from(i) * 0.01);
            many += dy;
            f = next;
        }
        assert!(one < 0.0 && (one - many).abs() < 1e-2, "{one} vs {many}");
    }
}
