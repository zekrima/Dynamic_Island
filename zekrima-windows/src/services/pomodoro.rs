//! Focus / Pomodoro countdown timer (zero background threads, 0% idle CPU).

use std::time::Instant;

pub const TIMER_PRESETS_SECS: [u32; 4] = [5 * 60, 15 * 60, 25 * 60, 45 * 60];

#[derive(Clone, Debug)]
pub struct PomodoroTimer {
    pub total_secs: u32,
    pub remaining_secs: u32,
    pub running: bool,
    pub completed: bool,
    last_tick: Option<Instant>,
}

impl Default for PomodoroTimer {
    fn default() -> Self {
        Self::new(25 * 60)
    }
}

impl PomodoroTimer {
    pub fn new(secs: u32) -> Self {
        Self {
            total_secs: secs.max(1),
            remaining_secs: secs.max(1),
            running: false,
            completed: false,
            last_tick: None,
        }
    }

    pub fn select_preset(&mut self, secs: u32) {
        self.total_secs = secs.max(1);
        self.remaining_secs = self.total_secs;
        self.running = false;
        self.completed = false;
        self.last_tick = None;
    }

    pub fn toggle_start_pause(&mut self) {
        if self.running {
            self.running = false;
            self.last_tick = None;
        } else {
            if self.remaining_secs == 0 {
                self.remaining_secs = self.total_secs;
            }
            self.completed = false;
            self.running = true;
            self.last_tick = Some(Instant::now());
        }
    }

    pub fn reset(&mut self) {
        self.remaining_secs = self.total_secs;
        self.running = false;
        self.completed = false;
        self.last_tick = None;
    }

    /// Advances the timer using wall-clock elapsed time. Returns `true` on the
    /// exact tick when the countdown hits `00:00`.
    pub fn tick(&mut self) -> bool {
        if !self.running {
            return false;
        }
        let now = Instant::now();
        let elapsed = self
            .last_tick
            .map(|prev| now.saturating_duration_since(prev).as_secs() as u32)
            .unwrap_or(0);
        if elapsed == 0 {
            return false;
        }
        self.last_tick = Some(now);
        self.remaining_secs = self.remaining_secs.saturating_sub(elapsed);
        if self.remaining_secs == 0 {
            self.running = false;
            self.completed = true;
            self.last_tick = None;
            return true;
        }
        false
    }

    pub fn progress(&self) -> f32 {
        if self.total_secs == 0 {
            return 0.0;
        }
        (self.remaining_secs as f32 / self.total_secs as f32).clamp(0.0, 1.0)
    }

    pub fn formatted(&self) -> String {
        let mins = self.remaining_secs / 60;
        let secs = self.remaining_secs % 60;
        format!("{mins:02}:{secs:02}")
    }
}
