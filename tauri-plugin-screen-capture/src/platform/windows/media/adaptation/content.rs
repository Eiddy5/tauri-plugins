use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const ACTIVITY_WINDOW: Duration = Duration::from_secs(2);
const MOTION_CONFIRMATION: Duration = Duration::from_millis(600);
const STATIC_CONFIRMATION: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentState {
    Static,
    Interactive,
    Motion,
}

impl ContentState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Interactive => "interactive",
            Self::Motion => "motion",
        }
    }
}

pub(crate) struct ContentActivityTracker {
    fresh_frames: VecDeque<Instant>,
    state: ContentState,
    high_activity_since: Option<Instant>,
    low_activity_since: Option<Instant>,
}

impl Default for ContentActivityTracker {
    fn default() -> Self {
        Self {
            fresh_frames: VecDeque::new(),
            state: ContentState::Interactive,
            high_activity_since: None,
            low_activity_since: None,
        }
    }
}

impl ContentActivityTracker {
    pub(crate) fn observe(&mut self, fresh: bool, requested_fps: u32, now: Instant) {
        if fresh {
            self.fresh_frames.push_back(now);
        }
        while self
            .fresh_frames
            .front()
            .is_some_and(|sample| now.duration_since(*sample) > ACTIVITY_WINDOW)
        {
            self.fresh_frames.pop_front();
        }

        let expected = f64::from(requested_fps.max(1)) * ACTIVITY_WINDOW.as_secs_f64();
        let activity = self.fresh_frames.len() as f64 / expected;
        if activity >= 0.55 {
            self.low_activity_since = None;
            let since = self.high_activity_since.get_or_insert(now);
            if now.duration_since(*since) >= MOTION_CONFIRMATION {
                self.state = ContentState::Motion;
            }
        } else if activity <= 0.08 {
            self.high_activity_since = None;
            let since = self.low_activity_since.get_or_insert(now);
            if now.duration_since(*since) >= STATIC_CONFIRMATION {
                self.state = ContentState::Static;
            }
        } else {
            self.high_activity_since = None;
            self.low_activity_since = None;
            self.state = ContentState::Interactive;
        }
    }

    pub(crate) fn state(&self) -> ContentState {
        self.state
    }
}
