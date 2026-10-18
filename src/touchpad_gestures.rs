#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub id: u8,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    pub gestures: bool,
    pub tap_click: bool,
    pub accel: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swipe {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Output {
    Move(f32, f32),
    Scroll(f32, f32),
    Press(MouseButton),
    Release(MouseButton),
    Click(MouseButton),
    Swipe(Swipe),
}

pub const SETTLE_SECS: f32 = 0.03;
pub const MIN_CONTACT_SECS: f32 = 0.02;
pub const TAP_MAX_SECS: f32 = 0.25;
pub const TAP_MAX_TRAVEL: f32 = 0.04;
pub const TAP_DRAG_WINDOW_SECS: f32 = 0.2;
pub const EDGE_MARGIN: f32 = 0.02;
pub const RIGHT_ZONE_X: f32 = 2.0 / 3.0;
pub const SWIPE_DIST: f32 = 0.15;
pub const COAST_TAU_SECS: f32 = 0.25;
pub const COAST_START_SPEED: f32 = 0.3;
pub const COAST_STOP_SPEED: f32 = 0.05;
const VEL_EMA: f32 = 0.5;

pub fn accel_gain(speed: f32, accel: f32) -> f32 {
    (1.0 + (accel - 1.0).max(0.0) * 0.5 * speed.max(0.0)).clamp(1.0, 4.0)
}

pub fn click_button(x: f32) -> MouseButton {
    if x >= RIGHT_ZONE_X {
        MouseButton::Right
    } else {
        MouseButton::Left
    }
}

pub fn swipe_dir(dx: f32, dy: f32) -> Option<Swipe> {
    if dx.abs().max(dy.abs()) < SWIPE_DIST {
        return None;
    }
    Some(if dx.abs() >= dy.abs() {
        if dx > 0.0 {
            Swipe::Right
        } else {
            Swipe::Left
        }
    } else if dy > 0.0 {
        Swipe::Down
    } else {
        Swipe::Up
    })
}

fn on_edge(c: &Contact) -> bool {
    c.x < EDGE_MARGIN || c.x > 1.0 - EDGE_MARGIN || c.y < EDGE_MARGIN || c.y > 1.0 - EDGE_MARGIN
}

#[derive(Debug)]
struct Session {
    age: f32,
    settle_until: f32,
    max_fingers: usize,
    last: Vec<Contact>,
    travel: f32,
    edge_start: bool,
    clicked: bool,
    tap_drag: bool,
    scrolled: bool,
}

#[derive(Debug)]
struct ClickHold {
    button: MouseButton,
    origin: Option<(f32, f32)>,
    swiped: bool,
}

#[derive(Debug, Default)]
pub struct TouchpadGestures {
    session: Option<Session>,
    pending_tap: Option<f32>,
    click: Option<ClickHold>,
    scroll_vel: (f32, f32),
    coast: (f32, f32),
}

impl TouchpadGestures {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn step(
        &mut self,
        contacts: &[Contact],
        click: bool,
        dt: f32,
        cfg: &Config,
    ) -> Vec<Output> {
        let mut out = Vec::new();
        let contacts = &contacts[..contacts.len().min(2)];
        let click = click && cfg.gestures;

        if click && self.click.is_none() {
            let pos = contacts.first().map(|c| (c.x, c.y)).or_else(|| {
                self.session
                    .as_ref()
                    .and_then(|s| s.last.first())
                    .map(|c| (c.x, c.y))
            });
            if self.pending_tap.take().is_some() {
                out.push(Output::Click(MouseButton::Left));
            }
            self.click = Some(ClickHold {
                button: pos.map_or(MouseButton::Left, |p| click_button(p.0)),
                origin: pos,
                swiped: false,
            });
            self.coast = (0.0, 0.0);
            if let Some(s) = self.session.as_mut() {
                s.clicked = true;
            }
        } else if !click {
            if let Some(c) = self.click.take() {
                if !c.swiped {
                    out.push(Output::Click(c.button));
                }
            }
        }

        if contacts.is_empty() {
            if let Some(s) = self.session.take() {
                self.end_session(s, cfg, &mut out);
            } else if let Some(age) = self.pending_tap.as_mut() {
                *age += dt;
                if *age > TAP_DRAG_WINDOW_SECS {
                    self.pending_tap = None;
                    out.push(Output::Click(MouseButton::Left));
                }
            }
            if self.coast != (0.0, 0.0) {
                out.push(Output::Scroll(self.coast.0 * dt, self.coast.1 * dt));
                let k = (-dt / COAST_TAU_SECS).exp();
                self.coast = (self.coast.0 * k, self.coast.1 * k);
                if self.coast.0.hypot(self.coast.1) < COAST_STOP_SPEED {
                    self.coast = (0.0, 0.0);
                }
            }
            return out;
        }

        self.coast = (0.0, 0.0);
        if self.session.is_none() {
            let tap_drag = cfg.tap_click && self.pending_tap.take().is_some();
            if tap_drag {
                out.push(Output::Press(MouseButton::Left));
            }
            self.scroll_vel = (0.0, 0.0);
            self.session = Some(Session {
                age: 0.0,
                settle_until: SETTLE_SECS,
                max_fingers: 0,
                last: Vec::new(),
                travel: 0.0,
                edge_start: contacts.iter().any(on_edge),
                clicked: self.click.is_some(),
                tap_drag,
                scrolled: false,
            });
        }
        let s = self.session.as_mut().expect("session");
        s.age += dt;
        s.max_fingers = s.max_fingers.max(contacts.len());

        let same_ids = s.last.len() == contacts.len()
            && contacts.iter().all(|c| s.last.iter().any(|l| l.id == c.id));
        if !same_ids {
            if !s.last.is_empty() {
                s.settle_until = s.age + SETTLE_SECS;
            }
            s.last = contacts.to_vec();
            return out;
        }
        let n = contacts.len() as f32;
        let (mut dx, mut dy) = (0.0, 0.0);
        for c in contacts {
            let l = s.last.iter().find(|l| l.id == c.id).expect("matched id");
            dx += (c.x - l.x) / n;
            dy += (c.y - l.y) / n;
        }
        s.last = contacts.to_vec();
        s.travel += dx.hypot(dy);
        if s.age < s.settle_until {
            return out;
        }

        if let Some(hold) = self.click.as_mut() {
            let primary = contacts[0];
            let origin = *hold.origin.get_or_insert((primary.x, primary.y));
            if !hold.swiped && contacts.len() == 1 {
                if let Some(dir) = swipe_dir(primary.x - origin.0, primary.y - origin.1) {
                    hold.swiped = true;
                    out.push(Output::Swipe(dir));
                }
            }
            return out;
        }

        if s.max_fingers >= 2 {
            if cfg.gestures && contacts.len() == 2 {
                if dt > 0.0 {
                    self.scroll_vel = (
                        self.scroll_vel.0 * VEL_EMA + dx / dt * (1.0 - VEL_EMA),
                        self.scroll_vel.1 * VEL_EMA + dy / dt * (1.0 - VEL_EMA),
                    );
                }
                if dx != 0.0 || dy != 0.0 {
                    s.scrolled = true;
                    out.push(Output::Scroll(dx, dy));
                }
            }
            return out;
        }

        if dx != 0.0 || dy != 0.0 {
            let speed = if dt > 0.0 { dx.hypot(dy) / dt } else { 0.0 };
            let g = accel_gain(speed, cfg.accel);
            out.push(Output::Move(dx * g, dy * g));
        }
        out
    }

    fn end_session(&mut self, s: Session, cfg: &Config, out: &mut Vec<Output>) {
        let tap = !s.clicked
            && !s.edge_start
            && s.age >= MIN_CONTACT_SECS
            && s.age <= TAP_MAX_SECS
            && s.travel <= TAP_MAX_TRAVEL;
        if s.tap_drag {
            out.push(Output::Release(MouseButton::Left));
            if tap {
                out.push(Output::Click(MouseButton::Left));
            }
            return;
        }
        if s.max_fingers >= 2 {
            if tap && cfg.tap_click && !s.scrolled {
                out.push(Output::Click(MouseButton::Right));
            }
            if cfg.gestures
                && s.scrolled
                && self.scroll_vel.0.hypot(self.scroll_vel.1) >= COAST_START_SPEED
            {
                self.coast = self.scroll_vel;
            }
            return;
        }
        if tap && cfg.tap_click {
            self.pending_tap = Some(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 0.008;
    const CFG: Config = Config {
        gestures: true,
        tap_click: true,
        accel: 2.0,
    };

    fn c(id: u8, x: f32, y: f32) -> Contact {
        Contact { id, x, y }
    }

    struct Rig {
        g: TouchpadGestures,
        cfg: Config,
        out: Vec<Output>,
    }

    impl Rig {
        fn new() -> Self {
            Self::with(CFG)
        }
        fn with(cfg: Config) -> Self {
            Self {
                g: TouchpadGestures::new(),
                cfg,
                out: Vec::new(),
            }
        }
        fn frame(&mut self, contacts: &[Contact], click: bool) {
            let o = self.g.step(contacts, click, DT, &self.cfg);
            self.out.extend(o);
        }
        fn hold(&mut self, contacts: &[Contact], click: bool, secs: f32) {
            for _ in 0..(secs / DT).round() as usize {
                self.frame(contacts, click);
            }
        }
        fn idle(&mut self, secs: f32) {
            self.hold(&[], false, secs);
        }
        fn slide(&mut self, from: &[Contact], dx: f32, dy: f32, frames: usize, click: bool) {
            for i in 0..=frames {
                let t = i as f32 / frames as f32;
                let f: Vec<Contact> = from
                    .iter()
                    .map(|p| c(p.id, p.x + dx * t, p.y + dy * t))
                    .collect();
                self.frame(&f, click);
            }
        }
        fn take(&mut self) -> Vec<Output> {
            std::mem::take(&mut self.out)
        }
        fn clicks(&self) -> Vec<MouseButton> {
            self.out
                .iter()
                .filter_map(|o| match o {
                    Output::Click(b) => Some(*b),
                    _ => None,
                })
                .collect()
        }
        fn moved(&self) -> (f32, f32) {
            self.out.iter().fold((0.0, 0.0), |a, o| match o {
                Output::Move(x, y) => (a.0 + x, a.1 + y),
                _ => a,
            })
        }
        fn scrolled(&self) -> (f32, f32) {
            self.out.iter().fold((0.0, 0.0), |a, o| match o {
                Output::Scroll(x, y) => (a.0 + x, a.1 + y),
                _ => a,
            })
        }
    }

    #[test]
    fn one_finger_move_follows_finger_after_settle() {
        let mut r = Rig::new();
        r.slide(&[c(0, 0.3, 0.5)], 0.2, -0.1, 40, false);
        let (x, y) = r.moved();
        assert!(x > 0.15 && x < 0.2 * 4.0, "{x}");
        assert!(y < -0.05, "{y}");
        r.idle(0.5);
        assert!(r.clicks().is_empty());
    }

    #[test]
    fn first_frames_after_touch_down_are_ignored() {
        let mut r = Rig::new();
        r.frame(&[c(0, 0.5, 0.5)], false);
        r.frame(&[c(0, 0.6, 0.5)], false);
        r.frame(&[c(0, 0.7, 0.5)], false);
        assert_eq!(r.moved(), (0.0, 0.0));
    }

    #[test]
    fn faster_motion_gains_more() {
        assert_eq!(accel_gain(0.0, 2.0), 1.0);
        assert!(accel_gain(2.0, 2.0) > accel_gain(0.5, 2.0));
        assert_eq!(accel_gain(5.0, 1.0), 1.0);
        assert_eq!(accel_gain(100.0, 3.0), 4.0);
    }

    #[test]
    fn tap_clicks_left_after_drag_window() {
        let mut r = Rig::new();
        r.hold(&[c(0, 0.5, 0.5)], false, 0.08);
        r.frame(&[], false);
        assert!(r.clicks().is_empty());
        r.idle(TAP_DRAG_WINDOW_SECS + 0.02);
        assert_eq!(r.clicks(), vec![MouseButton::Left]);
    }

    #[test]
    fn tap_click_off_suppresses_taps() {
        let mut r = Rig::with(Config {
            tap_click: false,
            ..CFG
        });
        r.hold(&[c(0, 0.5, 0.5)], false, 0.08);
        r.idle(0.5);
        r.hold(&[c(0, 0.5, 0.5), c(1, 0.6, 0.5)], false, 0.08);
        r.idle(0.5);
        assert!(r.out.is_empty(), "{:?}", r.out);
    }

    #[test]
    fn single_frame_blip_is_not_a_tap() {
        let mut r = Rig::new();
        r.frame(&[c(0, 0.5, 0.5)], false);
        r.idle(0.5);
        assert!(r.out.is_empty(), "{:?}", r.out);
    }

    #[test]
    fn edge_contact_is_not_a_tap() {
        let mut r = Rig::new();
        r.hold(&[c(0, 0.005, 0.5)], false, 0.08);
        r.idle(0.5);
        assert!(r.clicks().is_empty());
    }

    #[test]
    fn long_rest_is_not_a_tap() {
        let mut r = Rig::new();
        r.hold(&[c(0, 0.5, 0.5)], false, 0.6);
        r.idle(0.5);
        assert!(r.clicks().is_empty());
    }

    #[test]
    fn moving_contact_is_not_a_tap() {
        let mut r = Rig::new();
        r.slide(&[c(0, 0.3, 0.5)], 0.1, 0.0, 15, false);
        r.idle(0.5);
        assert!(r.clicks().is_empty());
    }

    #[test]
    fn two_finger_tap_right_clicks() {
        let mut r = Rig::new();
        r.hold(&[c(0, 0.4, 0.5)], false, 0.016);
        r.hold(&[c(0, 0.4, 0.5), c(1, 0.6, 0.5)], false, 0.08);
        r.frame(&[], false);
        r.idle(0.5);
        assert_eq!(r.clicks(), vec![MouseButton::Right]);
        assert_eq!(r.moved(), (0.0, 0.0));
    }

    #[test]
    fn two_finger_drag_scrolls_both_axes_then_coasts() {
        let mut r = Rig::new();
        r.slide(&[c(0, 0.4, 0.7), c(1, 0.6, 0.7)], 0.0, -0.4, 30, false);
        let during = r.scrolled();
        assert!(during.1 < -0.3, "{during:?}");
        assert_eq!(r.moved(), (0.0, 0.0));
        r.take();
        r.frame(&[], false);
        r.idle(1.0);
        let coast = r.scrolled();
        assert!(coast.1 < -0.01, "{coast:?}");
        r.take();
        r.idle(0.5);
        assert_eq!(r.scrolled(), (0.0, 0.0));
        assert!(r.clicks().is_empty());

        let mut r = Rig::new();
        r.slide(&[c(0, 0.3, 0.4), c(1, 0.3, 0.6)], 0.3, 0.0, 30, false);
        assert!(r.scrolled().0 > 0.2);
    }

    #[test]
    fn slow_two_finger_lift_does_not_coast() {
        let mut r = Rig::new();
        r.slide(&[c(0, 0.4, 0.7), c(1, 0.6, 0.7)], 0.0, -0.1, 30, false);
        r.hold(&[c(0, 0.4, 0.6), c(1, 0.6, 0.6)], false, 0.2);
        r.take();
        r.idle(0.5);
        assert_eq!(r.scrolled(), (0.0, 0.0));
    }

    #[test]
    fn new_touch_stops_coast() {
        let mut r = Rig::new();
        r.slide(&[c(0, 0.4, 0.7), c(1, 0.6, 0.7)], 0.0, -0.4, 30, false);
        r.frame(&[], false);
        r.take();
        r.frame(&[c(0, 0.5, 0.5)], false);
        r.hold(&[c(0, 0.5, 0.5)], false, 0.1);
        assert_eq!(r.scrolled(), (0.0, 0.0));
    }

    #[test]
    fn scroll_gestures_off_disables_scroll_but_not_move() {
        let mut r = Rig::with(Config {
            gestures: false,
            ..CFG
        });
        r.slide(&[c(0, 0.4, 0.7), c(1, 0.6, 0.7)], 0.0, -0.4, 30, false);
        assert_eq!(r.scrolled(), (0.0, 0.0));
        r.idle(0.5);
        r.slide(&[c(0, 0.3, 0.5)], 0.2, 0.0, 30, false);
        assert!(r.moved().0 > 0.1);
    }

    #[test]
    fn physical_click_left_and_right_third() {
        let mut r = Rig::new();
        r.hold(&[c(0, 0.3, 0.5)], false, 0.05);
        r.hold(&[c(0, 0.3, 0.5)], true, 0.1);
        r.frame(&[c(0, 0.3, 0.5)], false);
        r.frame(&[], false);
        r.idle(0.5);
        assert_eq!(r.clicks(), vec![MouseButton::Left]);

        let mut r = Rig::new();
        r.hold(&[c(0, 0.85, 0.5)], false, 0.05);
        r.hold(&[c(0, 0.85, 0.5)], true, 0.1);
        r.frame(&[c(0, 0.85, 0.5)], false);
        r.frame(&[], false);
        r.idle(0.5);
        assert_eq!(r.clicks(), vec![MouseButton::Right]);
    }

    #[test]
    fn physical_click_without_finger_is_left() {
        let mut r = Rig::new();
        r.hold(&[], true, 0.05);
        r.frame(&[], false);
        assert_eq!(r.clicks(), vec![MouseButton::Left]);
    }

    #[test]
    fn click_held_swipes_map_to_desktop_gestures() {
        for (dx, dy, want) in [
            (-0.3, 0.0, Swipe::Left),
            (0.3, 0.0, Swipe::Right),
            (0.0, -0.3, Swipe::Up),
            (0.0, 0.3, Swipe::Down),
        ] {
            let mut r = Rig::new();
            r.hold(&[c(0, 0.5, 0.5)], false, 0.05);
            r.hold(&[c(0, 0.5, 0.5)], true, 0.05);
            r.slide(&[c(0, 0.5, 0.5)], dx, dy, 20, true);
            r.frame(&[c(0, 0.5 + dx, 0.5 + dy)], false);
            r.frame(&[], false);
            r.idle(0.5);
            let swipes: Vec<_> = r
                .out
                .iter()
                .filter(|o| matches!(o, Output::Swipe(_)))
                .collect();
            assert_eq!(swipes, vec![&Output::Swipe(want)]);
            assert!(r.clicks().is_empty());
            assert_eq!(r.moved(), (0.0, 0.0));
        }
    }

    #[test]
    fn click_gestures_off_leaves_click_to_caller() {
        let mut r = Rig::with(Config {
            gestures: false,
            ..CFG
        });
        r.hold(&[c(0, 0.85, 0.5)], true, 0.1);
        r.slide(&[c(0, 0.85, 0.5)], -0.4, 0.0, 20, true);
        r.frame(&[], false);
        r.idle(0.5);
        assert!(
            r.out.iter().all(|o| matches!(o, Output::Move(..))),
            "{:?}",
            r.out
        );
    }

    #[test]
    fn tap_then_drag_holds_left_button() {
        let mut r = Rig::new();
        r.hold(&[c(0, 0.5, 0.5)], false, 0.08);
        r.frame(&[], false);
        r.idle(0.08);
        r.slide(&[c(0, 0.5, 0.5)], 0.2, 0.0, 30, false);
        r.frame(&[], false);
        r.idle(0.5);
        assert_eq!(r.out.first(), Some(&Output::Press(MouseButton::Left)));
        assert!(r.out.contains(&Output::Release(MouseButton::Left)));
        assert!(r.moved().0 > 0.1);
        assert!(r.clicks().is_empty());
    }

    #[test]
    fn double_tap_is_double_click() {
        let mut r = Rig::new();
        r.hold(&[c(0, 0.5, 0.5)], false, 0.08);
        r.frame(&[], false);
        r.idle(0.08);
        r.hold(&[c(0, 0.5, 0.5)], false, 0.08);
        r.frame(&[], false);
        r.idle(0.5);
        assert_eq!(
            r.out,
            vec![
                Output::Press(MouseButton::Left),
                Output::Release(MouseButton::Left),
                Output::Click(MouseButton::Left),
            ]
        );
    }

    #[test]
    fn click_during_pending_tap_flushes_it() {
        let mut r = Rig::new();
        r.hold(&[c(0, 0.5, 0.5)], false, 0.08);
        r.frame(&[], false);
        r.frame(&[], true);
        r.frame(&[], false);
        assert_eq!(r.clicks(), vec![MouseButton::Left, MouseButton::Left]);
    }

    #[test]
    fn extra_fingers_are_ignored() {
        let mut r = Rig::new();
        r.slide(
            &[c(0, 0.4, 0.7), c(1, 0.6, 0.7), c(2, 0.9, 0.9)],
            0.0,
            -0.4,
            30,
            false,
        );
        assert!(r.scrolled().1 < -0.3);
    }

    #[test]
    fn swipe_dir_needs_distance() {
        assert_eq!(swipe_dir(0.1, 0.0), None);
        assert_eq!(swipe_dir(0.2, 0.1), Some(Swipe::Right));
        assert_eq!(swipe_dir(0.1, -0.2), Some(Swipe::Up));
        assert_eq!(click_button(0.7), MouseButton::Right);
        assert_eq!(click_button(0.6), MouseButton::Left);
    }
}
