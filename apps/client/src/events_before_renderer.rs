//! Session events applied while the first renderer is being built
//! (`RUSTCRAFT_OPT_MESH_DURING_RENDERER_INIT`, on by default; `=0`: they wait for the first frame).
//!
//! Building the renderer holds the main thread for a few hundred milliseconds (pipelines, block
//! textures) while the integrated server or the network already delivers the world. Between the
//! build phases, the events received are applied in their order as long as their handling neither
//! reads the renderer nor would act on it; the first other event stops the run and is applied first
//! at the next frame, so the sequence of applied events stays the same. The meshing of what they
//! brought then starts on the pool, as a frame would start it.

use rc_client::{EntityEvent, Event};

use crate::App;

pub(crate) fn enabled() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_MESH_DURING_RENDERER_INIT")
}

/// What the classification of an event needs to know about the app.
#[derive(Clone, Copy)]
struct EarlyContext {
    player_id: i32,
    /// Singleplayer: the "insecure server" toast is never due.
    solo: bool,
    /// Quick play log written at the entry into the world (it names the world with the interface's language).
    quick_play_log: bool,
    /// Sound engine ready or joinable without waiting (otherwise a sound would block the build).
    audio_ready: bool,
}

/// The event has, applied before the renderer exists, the effect it would have at the first frame.
/// Each kind listed here was checked: its handling reads neither the renderer nor its interface,
/// and leaves nothing a renderer under construction would miss (the dimension's shading and entity
/// lights are applied to it once built).
fn applies_without_renderer(ev: &Event, cx: EarlyContext) -> bool {
    match ev {
        // World data: grid, light, biomes, block entities, weather. A forgotten column has nothing to
        // remove from a renderer that does not exist yet.
        Event::Chunk(_)
        | Event::LightUpdate { .. }
        | Event::ChunkBiomes(_)
        | Event::ForgetChunk { .. }
        | Event::BlockUpdate { .. }
        | Event::SectionUpdate { .. }
        | Event::ChunkCenter { .. } => true,
        // State kept by the app and its parts (player, interface data, registries for the next frame).
        Event::Time { .. }
        | Event::Border(_)
        | Event::ViewDistance(_)
        | Event::SimulationDistance(_)
        | Event::Difficulty { .. }
        | Event::Abilities(_)
        | Event::HeldSlot(_)
        | Event::Health { .. }
        | Event::Experience { .. }
        | Event::Commands(_)
        | Event::Tags(_)
        | Event::Registry { .. }
        | Event::EnabledFeatures(_)
        | Event::TickingState { .. }
        | Event::TickingStep(_)
        | Event::PlayerInfo(_)
        | Event::Score(_)
        | Event::Chat { .. }
        | Event::Inventory(_)
        | Event::Attributes { .. }
        | Event::Teleport(_)
        | Event::Recipes(_)
        | Event::UpdateRecipes(_)
        | Event::LoginFinished
        | Event::LoginStatus(_)
        | Event::Skipped => true,
        // Weather, game mode, immediate respawn, chunks started: no sound, no message.
        Event::GameEvent { kind, .. } => matches!(kind, 1 | 2 | 3 | 7 | 8 | 11 | 13),
        // Data, effects and operator level of the local player (a sound may be played).
        Event::Entity(e) => cx.audio_ready && local_player_state(e, cx.player_id),
        // Entering the world joins the sound engine; outside singleplayer it may show a toast, and
        // the quick play log names the world in the interface's language.
        Event::Joined { .. } => cx.solo && !cx.quick_play_log && cx.audio_ready,
        Event::Bundle(events) => events.iter().all(|e| applies_without_renderer(e, cx)),
        _ => false,
    }
}

/// Entity event that only sets the local player's state (data, effects, operator level).
fn local_player_state(e: &EntityEvent, player_id: i32) -> bool {
    match e {
        EntityEvent::Event { id, event } => *id == player_id && (22..=28).contains(event),
        EntityEvent::Data { id, .. } | EntityEvent::Effect { id, .. } | EntityEvent::RemoveEffect { id, .. } => {
            *id == player_id
        }
        _ => false,
    }
}

/// Applies, in order, at most `count` events taken by `next` while `applies` accepts them. The first
/// refused one is handed back to be applied first at the next frame; the rest stays queued.
fn apply_in_order<S, E>(
    state: &mut S,
    count: usize,
    next: fn(&mut S) -> Option<E>,
    applies: fn(&S, &E) -> bool,
    apply: fn(&mut S, E),
) -> Option<E> {
    for _ in 0..count {
        let ev = next(state)?;
        if !applies(state, &ev) {
            return Some(ev);
        }
        apply(state, ev);
    }
    None
}

impl App {
    fn early_context(&self) -> EarlyContext {
        EarlyContext {
            player_id: self.player_id,
            solo: self.solo.is_some(),
            quick_play_log: self.quick_log.enabled(),
            audio_ready: self.audio.joins_without_waiting(),
        }
    }

    /// Called between the phases of the first renderer's build: applies the events already received
    /// that need no renderer, then starts their meshing like a frame.
    pub(crate) fn apply_events_before_renderer(&mut self) {
        // Stopped at an event that needs the renderer: everything after it waits for the frame too.
        if self.held_event.is_some() {
            return;
        }
        let count = self.session.events.len();
        self.held_event = apply_in_order(
            self,
            count,
            |app| app.session.events.try_recv().ok(),
            |app, ev| applies_without_renderer(ev, app.early_context()),
            App::deliver,
        );
        if self.front.in_world {
            // The meshing settings come from the options before the first task (set again, unchanged,
            // when the renderer applies the options).
            self.refresh_mesh_options();
            self.relight_then_schedule_meshing();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Queue of numbered events; odd numbers need the renderer.
    struct Trace {
        queue: VecDeque<u32>,
        applied: Vec<u32>,
    }

    fn run(t: &mut Trace, count: usize) -> Option<u32> {
        apply_in_order(t, count, |t| t.queue.pop_front(), |_, &e| e % 2 == 0, |t, e| t.applied.push(e))
    }

    /// The events applied during the build, then the held one and the rest at the first frame, come
    /// in the order received. Sabotage checked: skipping the refused event instead of stopping
    /// (applying 0, 2, 6 then 3) fails the order.
    #[test]
    fn events_applied_in_the_order_received() {
        let mut t = Trace { queue: VecDeque::from([0, 2, 3, 6, 8]), applied: Vec::new() };
        let held = run(&mut t, 10);
        assert_eq!((t.applied.as_slice(), held), (&[0, 2][..], Some(3)), "stops at the first that needs the renderer");
        assert_eq!(t.queue, [6, 8], "the rest stays queued");
        // First frame: the held event first, then the queue.
        t.applied.extend(held);
        t.applied.extend(t.queue.drain(..));
        assert_eq!(t.applied, [0, 2, 3, 6, 8]);
    }

    /// No more than the events present when called (a flood does not hold the build), and an empty
    /// queue holds nothing back.
    #[test]
    fn bounded_by_the_events_present() {
        let mut t = Trace { queue: VecDeque::from([0, 2, 4, 6]), applied: Vec::new() };
        assert_eq!(run(&mut t, 2), None);
        assert_eq!((t.applied.as_slice(), t.queue.len()), (&[0, 2][..], 2));
        assert_eq!(run(&mut t, 5), None);
        assert_eq!(t.applied, [0, 2, 4, 6]);
    }

    fn cx() -> EarlyContext {
        EarlyContext { player_id: 7, solo: true, quick_play_log: false, audio_ready: true }
    }

    /// Kinds that would act on the renderer or its interface wait for it; the local player's state
    /// and world data do not. Sabotage checked: accepting every entity event fails the third line.
    #[test]
    fn kinds_that_need_the_renderer_wait_for_it() {
        let time = Event::Time { game_time: 1, clocks: Vec::new() };
        assert!(applies_without_renderer(&time, cx()));
        assert!(applies_without_renderer(&Event::HeldSlot(3), cx()));
        let other = Event::Entity(EntityEvent::Event { id: 8, event: 24 });
        assert!(!applies_without_renderer(&other, cx()), "another entity's event");
        let op = Event::Entity(EntityEvent::Event { id: 7, event: 24 });
        assert!(applies_without_renderer(&op, cx()));
        assert!(!applies_without_renderer(&op, EarlyContext { audio_ready: false, ..cx() }), "sound engine not ready");
        let totem = Event::Entity(EntityEvent::Event { id: 7, event: 35 });
        assert!(!applies_without_renderer(&totem, cx()), "totem animation");
        assert!(!applies_without_renderer(&Event::Disconnected("x".into()), cx()), "disconnect screen");
        assert!(!applies_without_renderer(&Event::GameEvent { kind: 5, param: 0.0 }, cx()), "demo message");
        assert!(applies_without_renderer(&Event::GameEvent { kind: 13, param: 0.0 }, cx()));
        let bundle = |last| Event::Bundle(vec![Event::HeldSlot(1), last]);
        assert!(applies_without_renderer(&bundle(Event::HeldSlot(2)), cx()));
        assert!(!applies_without_renderer(&bundle(Event::Disconnected("x".into())), cx()));
    }
}
