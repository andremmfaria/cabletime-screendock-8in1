//! The keyframe clock: when the daemon asks the encoder for a keyframe.

pub use super::types::KeyframeClock;

impl KeyframeClock {
    /// A clock that asks for a keyframe on the first frame and then every `interval` frames.
    /// An interval of 0 counts as 1.
    pub fn new(interval: u32) -> Self {
        Self {
            interval: interval.max(1),
            until_key: 0,
        }
    }

    /// The current spacing in frames.
    pub fn interval(&self) -> u32 {
        self.interval
    }

    /// Call once per frame before encoding it. True means the encoder must be asked for a
    /// keyframe for this frame.
    pub fn next_frame(&mut self) -> bool {
        if self.until_key == 0 {
            self.until_key = self.interval - 1;
            true
        } else {
            self.until_key -= 1;
            false
        }
    }

    /// Records that a keyframe was already requested for the next frame (after a failed
    /// write, a lost session or a rebuild), so the following one comes a full interval later.
    pub fn keyframe_forced(&mut self) {
        self.until_key = self.interval;
    }

    /// Changes the spacing. The next keyframe comes within the new interval.
    pub fn set_interval(&mut self, interval: u32) {
        self.interval = interval.max(1);
        self.until_key = self.until_key.min(self.interval - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_positions(clock: &mut KeyframeClock, frames: usize) -> Vec<usize> {
        let mut positions = Vec::new();
        for n in 0..frames {
            if clock.next_frame() {
                positions.push(n);
            }
        }
        positions
    }

    #[test]
    fn first_frame_and_then_every_interval_frames_are_keyframes() {
        let mut clock = KeyframeClock::new(4);
        assert_eq!(key_positions(&mut clock, 14), [0, 4, 8, 12]);
    }

    #[test]
    fn interval_of_one_makes_every_frame_a_keyframe() {
        let mut clock = KeyframeClock::new(1);
        assert_eq!(key_positions(&mut clock, 3), [0, 1, 2]);
        assert_eq!(KeyframeClock::new(0).interval(), 1);
    }

    #[test]
    fn forced_keyframe_restarts_the_count() {
        let mut clock = KeyframeClock::new(5);
        assert_eq!(key_positions(&mut clock, 3), [0]);
        // The caller forces a keyframe for the next frame, which is frame 0 of the new count.
        clock.keyframe_forced();
        assert_eq!(key_positions(&mut clock, 11), [5, 10]);
    }

    #[test]
    fn shorter_interval_takes_effect_within_the_new_interval() {
        let mut clock = KeyframeClock::new(50);
        assert_eq!(key_positions(&mut clock, 10), [0]);
        clock.set_interval(4);
        assert_eq!(key_positions(&mut clock, 12), [3, 7, 11]);
    }

    #[test]
    fn longer_interval_keeps_the_scheduled_keyframe_then_spaces_by_the_new_one() {
        let mut clock = KeyframeClock::new(5);
        assert_eq!(key_positions(&mut clock, 2), [0]);
        clock.set_interval(8);
        assert_eq!(key_positions(&mut clock, 20), [3, 11, 19]);
    }
}
