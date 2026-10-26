//! Demo mode (`--demo` launch option): welcome screen, first-day hints and day
//! messages (game event 5), then, past 120 500 game ticks, break, place and use denied
//! with a reminder at most every 101 ticks of finished demo.

use std::sync::atomic::{AtomicBool, Ordering};

use rc_client::Event;

use crate::server::Server;
use crate::tr;

/// Game time beyond which the demo is over (five days and a bit).
pub const TOTAL_PLAY_TICKS: i64 = 120_500;

/// Game event of the demo (value: 0 welcome, 101-103 hints, 104 end).
const DEMO_EVENT: u8 = 5;

static LAUNCH: AtomicBool = AtomicBool::new(false);

/// `--demo` launch option: worlds opened afterwards are opened in demo mode.
pub fn set_launch_demo(on: bool) {
    LAUNCH.store(on, Ordering::Relaxed);
}

pub(crate) fn launch_demo() -> bool {
    LAUNCH.load(Ordering::Relaxed)
}

/// Demo state of a player, new for each player created (login, respawn).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Demo {
    intro_sent: bool,
    pub(crate) ended: bool,
    /// Ticks of finished demo since the last reminder.
    reminder: i32,
    /// Ticks lived by this player.
    ticks: i32,
}

impl Server {
    /// Demo tick of the current player, after the mining one.
    pub(crate) fn demo_tick(&mut self) {
        if !self.demo_world {
            return;
        }
        let time = self.time;
        let day = time / 24000 + 1;
        let d = &mut self.demo;
        d.ticks += 1;
        let intro = !d.intro_sent && d.ticks > 20;
        d.intro_sent |= intro;
        d.ended = time > TOTAL_PLAY_TICKS;
        if d.ended {
            d.reminder += 1;
        }
        if intro {
            self.demo_event(0.0);
        }
        let in_day = time % 24000;
        if in_day == 500 {
            if day == 6 {
                self.demo_event(104.0);
            } else if day < 6 {
                self.system(tr!(&format!("demo.day.{day}")));
            }
        } else if day == 1 {
            match time {
                100 => self.demo_event(101.0),
                175 => self.demo_event(102.0),
                250 => self.demo_event(103.0),
                _ => {}
            }
        } else if day == 5 && in_day == 22000 {
            self.system(tr!("demo.day.warning"));
        }
    }

    fn demo_event(&self, param: f32) {
        let _ = self.send(Event::GameEvent { kind: DEMO_EVENT, param });
    }

    /// Demo over: the action (break, place, use) is denied, true; reminder if more than 100
    /// ticks of finished demo have passed since the previous one.
    pub(crate) fn demo_refuses(&mut self) -> bool {
        if !self.demo_world || !self.demo.ended {
            return false;
        }
        if self.demo.reminder > 100 {
            self.system(tr!("demo.reminder"));
            self.demo.reminder = 0;
        }
        true
    }
}
