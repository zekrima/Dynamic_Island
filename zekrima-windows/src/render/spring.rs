//! Damped harmonic spring physics used by the island geometry.
//!
//! Every animated value owns its own spring. A spring reports whether it is
//! still moving so the host window can stop its 16 ms timer as soon as the
//! whole island has settled. When nothing is moving the application is
//! completely idle: no timer, no repaint, 0.0% CPU.

/// Collapsed geometry (logical pixels).
pub const COLLAPSED_WIDTH: f32 = 178.0;
pub const COLLAPSED_BADGE_WIDTH: f32 = 228.0;
pub const COLLAPSED_HEIGHT: f32 = 34.0;
pub const COLLAPSED_RADIUS: f32 = 17.0;

/// HUD geometry (logical pixels) - shown for volume/brightness overlay.
pub const HUD_WIDTH: f32 = 300.0;
pub const HUD_HEIGHT: f32 = 52.0;
pub const HUD_RADIUS: f32 = 26.0;

/// Incoming notification toast geometry (logical pixels) - Pillar-style card.
pub const NOTIFICATION_WIDTH: f32 = 380.0;
pub const NOTIFICATION_HEIGHT: f32 = 88.0;
pub const NOTIFICATION_RADIUS: f32 = 26.0;

/// Expanded media/idle geometry (logical pixels).
pub const EXPANDED_WIDTH: f32 = 440.0;
pub const EXPANDED_HEIGHT: f32 = 120.0;
pub const EXPANDED_RADIUS: f32 = 28.0;

/// Expanded notifications list geometry (logical pixels).
pub const EXPANDED_NOTIFS_WIDTH: f32 = 440.0;
pub const EXPANDED_NOTIFS_HEIGHT: f32 = 162.0;
pub const EXPANDED_NOTIFS_RADIUS: f32 = 28.0;

/// Distance below which a spring is considered to be at rest.
const REST_DISTANCE: f32 = 0.05;
/// Velocity below which a spring is considered to be at rest.
const REST_VELOCITY: f32 = 0.05;

#[derive(Debug, Clone, Copy)]
pub struct Spring {
    current: f32,
    target: f32,
    velocity: f32,
    stiffness: f32,
    damping: f32,
}

impl Spring {
    pub fn new(initial: f32) -> Self {
        Self {
            current: initial,
            target: initial,
            velocity: 0.0,
            stiffness: 280.0,
            damping: 26.0,
        }
    }

    /// Changes the target. Returns `true` when the target actually changed.
    pub fn set_target(&mut self, target: f32) -> bool {
        if self.target == target {
            false
        } else {
            self.target = target;
            true
        }
    }

    /// Advances the spring and returns `true` while it is still moving.
    pub fn update(&mut self, dt: f32) -> bool {
        let force = (self.target - self.current) * self.stiffness;
        let damping_force = self.velocity * self.damping;

        self.velocity += (force - damping_force) * dt;
        self.current += self.velocity * dt;

        if (self.target - self.current).abs() < REST_DISTANCE && self.velocity.abs() < REST_VELOCITY
        {
            self.current = self.target;
            self.velocity = 0.0;
            false
        } else {
            true
        }
    }

    pub fn current(&self) -> f32 {
        self.current
    }

    /// `true` when the spring has reached its target and stopped moving.
    pub fn is_settled(&self) -> bool {
        self.current == self.target && self.velocity == 0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillMode {
    Collapsed,
    CollapsedWithBadge,
    Hud,
    Notification,
    Expanded,
    ExpandedNotifications,
}

/// The three animated dimensions of the pill.
#[derive(Debug, Clone, Copy)]
pub struct PillSprings {
    pub width: Spring,
    pub height: Spring,
    pub radius: Spring,
}

impl PillSprings {
    pub fn new() -> Self {
        Self {
            width: Spring::new(COLLAPSED_WIDTH),
            height: Spring::new(COLLAPSED_HEIGHT),
            radius: Spring::new(COLLAPSED_RADIUS),
        }
    }

    /// Sets the target geometry mode.
    pub fn set_mode(&mut self, mode: PillMode) -> bool {
        let (w, h, r) = match mode {
            PillMode::Collapsed => (COLLAPSED_WIDTH, COLLAPSED_HEIGHT, COLLAPSED_RADIUS),
            PillMode::CollapsedWithBadge => {
                (COLLAPSED_BADGE_WIDTH, COLLAPSED_HEIGHT, COLLAPSED_RADIUS)
            }
            PillMode::Hud => (HUD_WIDTH, HUD_HEIGHT, HUD_RADIUS),
            PillMode::Notification => (NOTIFICATION_WIDTH, NOTIFICATION_HEIGHT, NOTIFICATION_RADIUS),
            PillMode::Expanded => (EXPANDED_WIDTH, EXPANDED_HEIGHT, EXPANDED_RADIUS),
            PillMode::ExpandedNotifications => (
                EXPANDED_NOTIFS_WIDTH,
                EXPANDED_NOTIFS_HEIGHT,
                EXPANDED_NOTIFS_RADIUS,
            ),
        };
        let width_changed = self.width.set_target(w);
        let height_changed = self.height.set_target(h);
        let radius_changed = self.radius.set_target(r);
        width_changed || height_changed || radius_changed
    }

    /// Advances every spring. Returns `true` while any spring is moving.
    pub fn update(&mut self, dt: f32) -> bool {
        let width_moving = self.width.update(dt);
        let height_moving = self.height.update(dt);
        let radius_moving = self.radius.update(dt);

        width_moving || height_moving || radius_moving
    }

    /// Current `(width, height, corner_radius)` in logical pixels.
    pub fn geometry(&self) -> (f32, f32, f32) {
        (
            self.width.current(),
            self.height.current(),
            self.radius.current(),
        )
    }

    /// `true` when all three springs have settled.
    pub fn is_settled(&self) -> bool {
        self.width.is_settled() && self.height.is_settled() && self.radius.is_settled()
    }
}

impl Default for PillSprings {
    fn default() -> Self {
        Self::new()
    }
}
