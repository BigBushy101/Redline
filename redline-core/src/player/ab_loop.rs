//! A-B loop: two markers on the current track; on reaching B, playback goes back to A.

/// The shortest loop allowed, in seconds; anything shorter is mostly the seek's restart.
pub const MIN_LOOP: f64 = 0.5;

/// Loop markers in track seconds. Loops only once both are set.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AbLoop {
    pub a: Option<f64>,
    pub b: Option<f64>,
}

impl AbLoop {
    /// Puts A at `t`. A B that would no longer be after it is dropped.
    pub fn set_a(&mut self, t: f64) {
        let t = t.max(0.0);
        self.a = Some(t);
        if self.b.is_some_and(|b| b < t + MIN_LOOP) {
            self.b = None;
        }
    }

    /// Puts B at `t` (no further than `duration`), with A at the start if it isn't set.
    /// Returns false, changing nothing, if B would come before A or too close after it.
    pub fn set_b(&mut self, t: f64, duration: Option<f64>) -> bool {
        let t = duration.map_or(t, |d| t.min(d));
        let a = self.a.unwrap_or(0.0);
        if t < a + MIN_LOOP {
            return false;
        }
        self.a = Some(a);
        self.b = Some(t);
        true
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn is_set(&self) -> bool {
        self.a.is_some() || self.b.is_some()
    }

    pub fn is_active(&self) -> bool {
        self.a.is_some() && self.b.is_some()
    }

    /// Where to seek once playback is at `position`: back to A on reaching B.
    pub fn wrap(&self, position: f64) -> Option<f64> {
        match (self.a, self.b) {
            (Some(a), Some(b)) if position >= b => Some(a),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_back_to_a_on_reaching_b() {
        let mut l = AbLoop::default();
        l.set_a(10.0);
        assert_eq!(l.wrap(30.0), None, "A alone doesn't loop");
        assert!(l.set_b(20.0, Some(60.0)));
        assert!(l.is_active());
        assert_eq!(l.wrap(5.0), None, "before A plays on into the loop");
        assert_eq!(l.wrap(19.99), None);
        assert_eq!(l.wrap(20.0), Some(10.0));
        assert_eq!(l.wrap(20.03), Some(10.0), "a tick late still wraps");
        l.clear();
        assert_eq!(l.wrap(20.0), None);
        assert!(!l.is_set());
    }

    #[test]
    fn markers_stay_in_order() {
        let mut l = AbLoop::default();
        assert!(l.set_b(8.0, None), "B first: A is the start");
        assert_eq!((l.a, l.b), (Some(0.0), Some(8.0)));
        assert!(!l.set_b(0.2, None), "too close to A");
        assert_eq!(l.b, Some(8.0));
        l.set_a(9.0);
        assert_eq!((l.a, l.b), (Some(9.0), None), "A past B drops B");
        assert!(!l.set_b(5.0, None), "B before A is refused");
        assert_eq!(l.b, None);
        assert!(l.set_b(99.0, Some(30.0)));
        assert_eq!(l.b, Some(30.0), "B stops at the end");
        l.set_a(-3.0);
        assert_eq!(l.a, Some(0.0));
    }
}
