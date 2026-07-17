use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedulerDecision {
    Wait(Duration),
    Compose { source_generation: u64 },
    RepeatLast,
    DropBackpressure,
    Stop,
}

#[derive(Clone, Debug)]
pub struct SchedulerPolicy {
    target_interval: Duration,
    static_interval: Duration,
    deadline: Duration,
    latest_source: Option<u64>,
    latest_revision: u64,
    composed_source: Option<u64>,
    composed_revision: u64,
    has_output: bool,
    surface_available: bool,
    stopped: bool,
}

impl SchedulerPolicy {
    pub fn new(target_fps: u32, static_fps: u32, now: Duration) -> Self {
        Self {
            target_interval: interval(target_fps),
            static_interval: interval(static_fps),
            deadline: now,
            latest_source: None,
            latest_revision: 0,
            composed_source: None,
            composed_revision: 0,
            has_output: false,
            surface_available: true,
            stopped: false,
        }
    }

    pub fn observe_source(&mut self, generation: u64, _now: Duration) {
        self.latest_source = Some(generation);
    }

    pub fn observe_annotation_revision(&mut self, revision: u64) {
        self.latest_revision = revision;
    }

    pub fn set_surface_available(&mut self, available: bool) {
        self.surface_available = available;
    }

    pub fn invalidate_generation(&mut self, generation: u64) {
        if self
            .latest_source
            .is_some_and(|latest| latest <= generation)
        {
            self.latest_source = None;
        }
    }

    pub fn stop(&mut self) {
        self.stopped = true;
    }

    pub fn next(&self, now: Duration) -> SchedulerDecision {
        if self.stopped {
            return SchedulerDecision::Stop;
        }
        let Some(source_generation) = self.latest_source else {
            return SchedulerDecision::Wait(self.deadline.max(now));
        };
        let changed = self.composed_source != Some(source_generation)
            || self.composed_revision != self.latest_revision;
        if changed {
            return if self.surface_available {
                SchedulerDecision::Compose { source_generation }
            } else {
                SchedulerDecision::DropBackpressure
            };
        }
        if now < self.deadline {
            return SchedulerDecision::Wait(self.deadline);
        }
        if self.has_output {
            SchedulerDecision::RepeatLast
        } else {
            SchedulerDecision::Compose { source_generation }
        }
    }

    pub fn mark_composed(
        &mut self,
        source_generation: u64,
        annotation_revision: u64,
        now: Duration,
    ) {
        let changed = self.composed_source != Some(source_generation)
            || self.composed_revision != annotation_revision;
        self.composed_source = Some(source_generation);
        self.composed_revision = annotation_revision;
        self.has_output = true;
        self.deadline = now
            + if changed {
                self.target_interval
            } else {
                self.static_interval
            };
    }

    pub const fn deadline(&self) -> Duration {
        self.deadline
    }
}

fn interval(fps: u32) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(fps.max(1)))
}
