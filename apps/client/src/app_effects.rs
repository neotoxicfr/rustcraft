//! Client-side effects of packets and of the tick: particles, sounds and the audio tick.

use crate::{App, particles};

impl App {
    /// `level_particles` packet (`particles.md` § 1.1): an exact particle if the count is
    /// zero, otherwise Gaussian draws (default) or uniform ones (alternatives).
    pub(crate) fn level_particles(&mut self, p: &rc_proto_v777::entity::LevelParticles) {
        let Some(kind) = self.particles.kind_of(p.particle.kind) else { return };
        let o = particles::Options::from_net(&p.particle, p.force);
        let (d, v) = (p.delta.map(f64::from), p.speed.map(f64::from));
        let always = p.always_visible;
        if p.count == 0 {
            let vel = [v[0] * d[0], v[1] * d[1], v[2] * d[2]];
            self.particles.spawn_ex(kind, &o, p.pos, vel, always);
            return;
        }
        let r = &mut self.world_random;
        for _ in 0..p.count.min(16384) {
            let (pos, vel) = if p.mode == 0 {
                let off = [r.gaussian() * d[0], r.gaussian() * d[1], r.gaussian() * d[2]];
                let vel = [r.gaussian() * v[0], r.gaussian() * v[1], r.gaussian() * v[2]];
                (off, vel)
            } else {
                let off = [f64::from(r.f()) * d[0], f64::from(r.f()) * d[1], f64::from(r.f()) * d[2]];
                let vel = if p.mode == 2 {
                    [v[0] * f64::from(r.f()), v[1] * f64::from(r.f()), v[2] * f64::from(r.f())]
                } else {
                    v
                };
                (off, vel)
            };
            let at = [p.pos[0] + pos[0], p.pos[1] + pos[1], p.pos[2] + pos[2]];
            self.particles.spawn_ex(kind, &o, at, vel, always);
        }
        if o.vib.is_some() {
            // Vibration signal that follows an entity: oriented from its creation.
            let (ents, me) = (&self.entities, (self.player_id, self.player.pos));
            self.particles.track_vibrations(|id| if id == me.0 { Some(me.1) } else { ents.map.get(&id).map(|e| e.pos) });
        }
    }

    /// `sound` and `sound_entity` packets (`sounds.md` § 1.3, 1.4).
    pub(crate) fn play_sound(&mut self, s: rc_proto_v777::play::SoundPacket) {
        let Some(audio) = self.audio.get() else { return };
        let name = match &s.sound {
            rc_proto_v777::play::SoundRef::Named(n) => n.clone(),
            rc_proto_v777::play::SoundRef::Id(i) => {
                match self.data.registry("minecraft:sound_event").and_then(|r| r.entries.get(*i as usize)) {
                    Some(n) => n.clone(),
                    None => return,
                }
            }
        };
        let category = rc_audio::Category::from_id(s.category).unwrap_or(rc_audio::Category::Master);
        let pos = match s.entity {
            None => s.pos,
            Some(id) => {
                // Unknown or silent entity (flag 4 of the metadata): nothing.
                let Some(e) = self.entities.map.get(&id) else { return };
                if e.meta_bool(4) {
                    return;
                }
                Some(e.pos)
            }
        };
        let handle = audio.play(&name, category, pos, s.volume, s.pitch, Some(s.seed));
        if let (Some(h), Some(id)) = (handle, s.entity) {
            self.entity_sounds.push((h, id));
        }
    }

    /// Audio tick: sounds attached to the tracked entities (stopped if the entity disappears or goes quiet).
    pub(crate) fn tick_audio(&mut self) {
        self.watch_audio_devices();
        let Some(audio) = self.audio.get() else { return };
        let ents = &self.entities;
        let me = self.player.pos;
        self.entity_sounds.retain(|&(h, id)| {
            if !audio.is_playing(h) {
                return false;
            }
            match ents.sound_anchor(id, me) {
                Some(p) => {
                    audio.set_position(h, p.map(|v| f64::from(v as f32)));
                    true
                }
                None => {
                    audio.stop_handle(h);
                    false
                }
            }
        });
        audio.tick();
    }
}
