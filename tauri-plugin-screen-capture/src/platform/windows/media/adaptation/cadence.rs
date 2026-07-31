const SCENE_REFRESH_IDLE_TICKS: u32 = 6;

#[derive(Default)]
pub(crate) struct FrameCadence {
    repeated_ticks: u32,
}

impl FrameCadence {
    pub(crate) fn on_repeat_tick(&mut self) {
        self.repeated_ticks = self.repeated_ticks.saturating_add(1);
    }

    pub(crate) fn on_captured_frame(&mut self) -> bool {
        let refresh = self.repeated_ticks >= SCENE_REFRESH_IDLE_TICKS;
        self.repeated_ticks = 0;
        refresh
    }
}

#[cfg(test)]
mod tests {
    use super::FrameCadence;

    #[test]
    fn first_captured_frame_after_idle_requests_scene_refresh() {
        let mut cadence = FrameCadence::default();
        for _ in 0..6 {
            cadence.on_repeat_tick();
        }
        assert!(cadence.on_captured_frame());
        assert!(!cadence.on_captured_frame());
    }
}
