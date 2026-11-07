//! Camera placement and what the crosshair aims at: picking, reach, held item and block outline.

use crate::grid_world::aim_shape;
use crate::{App, camera, environ, hand, item_use, pick, pick_targets};

impl App {
    /// Camera and local player after the tick's physics (`hand-camera.md` § 3 to 5).
    pub(crate) fn tick_camera(&mut self) {
        let p = &self.player;
        let (dx, dz) = (p.pos[0] - self.prev_pos[0], p.pos[2] - self.prev_pos[2]);
        let dead = self.hud.dead();
        self.camera.tick(&camera::TickIn {
            eye_height: self.camera_eye_height().unwrap_or_else(|| p.eye_height()),
            flying: p.flying,
            movement_speed: p.attribute("minecraft:movement_speed").unwrap_or(0.1),
            on_ground: p.on_ground,
            swimming: p.swimming,
            dead,
            riding: false,
            moved: dx.hypot(dz),
            speed: p.vel[0].hypot(p.vel[2]),
            yaw: self.yaw,
            pitch: self.pitch,
        });
        self.entities.tick_me(p.pos, self.yaw, self.pitch, p.on_ground, dead);
        // Equipment of the local player: hands and armor taken from the inventory.
        if let Some(me) = &mut self.entities.me {
            // Mouse-free test: RUSTCRAFT_SWINGTEST swings the arm every 2 s.
            if me.age % 40 == 0 && rc_data::env_flag!("RUSTCRAFT_SWINGTEST") {
                me.swing = Some((0, 1, 6, 0));
            }
            me.scale = self.player.attribute("minecraft:scale").unwrap_or(1.0) as f32;
            let inv = &self.inventory;
            me.equipment[0] = inv.hotbar(self.hud.selected).cloned();
            me.equipment[1] = inv.offhand().cloned();
            for (slot, menu) in [(2, 8), (3, 7), (4, 6), (5, 5)] {
                me.equipment[slot] = inv.menu.get(menu).cloned().flatten();
            }
        }
        // Height of the hands (item swap, attack strength).
        let models = self.renderer.as_ref().and_then(|r| r.gui.items.as_ref()).map(|a| &a.models);
        let (ids, data) = (&self.component_ids, &self.data);
        let hand = self.camera.hand.get_or_insert_with(|| hand::HandState::new(data));
        hand.tick(
            [self.inventory.hotbar(self.hud.selected), self.inventory.offhand()],
            self.player.attribute("minecraft:attack_speed").unwrap_or(4.0),
            &|s| hand::swap_props(models, ids, data, s).0,
        );
    }

    /// Camera position of the frame (perspective, third-person pull-back).
    pub(crate) fn place_camera(&mut self, pt: f32) -> camera::Shot {
        if self.free_cam || !self.positioned {
            return camera::Shot { pos: self.eye, yaw: self.yaw, pitch: self.pitch };
        }
        if let Some(s) = self.bed_shot() {
            return s;
        }
        let feet: [f64; 3] =
            std::array::from_fn(|a| self.prev_pos[a] + (self.player.pos[a] - self.prev_pos[a]) * f64::from(pt));
        let scale = self.player.attribute("minecraft:scale").unwrap_or(1.0);
        let distance = if self.hud.dead() {
            4.0
        } else {
            let own = scale * self.player.attribute("minecraft:camera_distance").unwrap_or(4.0).clamp(0.0, 32.0);
            own.max(self.vehicle_camera_distance())
        };
        // Camera on another entity (`set_camera`): its feet and rotation. Boat rotation
        // added to the view: smoothed over the tick (`riding.rs`).
        let yaw = self.yaw - self.riding.turn * (1.0 - pt);
        let (feet, yaw, pitch) = self.camera_pose(pt).unwrap_or((feet, yaw, self.pitch));
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        self.camera.place(&self.data, pt, feet, yaw, pitch, distance, &|x, y, z| view.state(x, y, z))
    }

    /// Crosshair aiming (`targeting.md` § 1) from the camera's eyes.
    pub(crate) fn pick(&self) -> pick::Hit {
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let (yr, pr) = (self.yaw.to_radians(), self.pitch.to_radians());
        let dir = [f64::from(-yr.sin() * pr.cos()), f64::from(-pr.sin()), f64::from(yr.cos() * pr.cos())];
        let block_range = self.player.attribute("minecraft:block_interaction_range").unwrap_or(4.5);
        let entity_range = self.player.attribute("minecraft:entity_interaction_range").unwrap_or(3.0);
        let (data, full, held) = (&self.data, self.act.full_shape, self.held_name());
        let shape = |x: i32, y: i32, z: i32| -> pick::Shape<'_> {
            aim_shape(data, full, held, view.state(x, y, z), [x, y, z])
                .map_or_else(|| pick::plain(&[]), |(i, shift)| (data.shape(i), shift))
        };
        // Interaction shape (cauldrons, composter, hopper, scaffolding): aimed face.
        let interaction = |x: i32, y: i32, z: i32| rc_physics::ray::interaction_shape(data, view.state(x, y, z));
        // Active item (in use, otherwise main hand; nothing in spectator) within
        // attack reach: its aiming goes first, player movement included along the gaze.
        let attack = (self.env.game_mode != 3).then(|| self.attack_reach()).flatten();
        let along: f64 = (0..3).map(|i| self.player.vel[i] * dir[i]).sum();
        let camera_box = self.camera_box();
        // Widest search box of the frame: normal targeting over its whole reach, attack-range
        // targeting over its whole segment; each one keeps the entities of its own box, shortened
        // to the block hit.
        let mut widest = pick::gaze_search_box(&camera_box, dir, block_range.max(entity_range));
        if let Some(a) = attack {
            let at = |k: f64| -> [f64; 3] { std::array::from_fn(|i| self.eye[i] + dir[i] * k) };
            let attack_box = pick::attack_search_box(at(a.min), at(a.max + along.max(0.0)), a.margin);
            widest = pick::hull(&widest, &attack_box);
        }
        let area = pick_targets::TargetArea::of(&widest, self.camera_root());
        let candidates = || self.pick_candidates(area);
        if let Some(a) = attack {
            let border = self.weather.border.bounds();
            let hit =
                pick::pick_attack(self.eye, dir, a, along, block_range, border, &shape, &interaction, candidates());
            if hit != pick::Hit::Miss {
                return hit;
            }
        }
        pick::pick(self.eye, dir, &camera_box, block_range, entity_range, &shape, &interaction, candidates())
    }

    /// Current box of the camera entity (the local player): the game's entity searches start
    /// from it.
    pub(crate) fn camera_box(&self) -> rc_data::Aabb {
        let b = self.player.bounding_box();
        rc_data::Aabb { min: b.min, max: b.max }
    }

    /// Root vehicle of the camera entity: the local player's, or the player when riding nothing.
    pub(crate) fn camera_root(&self) -> i32 {
        self.player_vehicle().map_or(self.player_id, |v| self.root_vehicle(v))
    }

    /// Targetable entities of `area`: current box, selection radius, same root vehicle as the
    /// camera.
    pub(crate) fn pick_candidates(&self, area: pick_targets::TargetArea) -> impl Iterator<Item = pick::Candidate> + '_ {
        let ents = self.renderer.as_ref().and_then(|r| r.entities.as_ref());
        let source = pick_targets::TargetSource {
            ents: &self.entities,
            players: &self.players,
            table: ents.map_or(&[][..], |e| e.dims.as_slice()),
            painting: move |v: i32| ents.and_then(|e| e.painting_size(v)),
            component_ids: &self.component_ids,
        };
        pick_targets::candidates(source, area, pick_targets::TargetPaths::current())
    }

    /// Attack reach of the active item (`attack_range`): the one in use, otherwise the main
    /// hand; ranges of creative mode or of the other modes.
    fn attack_reach(&self) -> Option<pick::AttackReach> {
        let stack = match self.act.using.as_ref() {
            Some(u) if u.hand == 1 => self.inventory.offhand(),
            _ => self.inventory.hotbar(self.hud.selected),
        }?;
        let r = item_use::ItemView::new(stack, &self.data, &self.act.ids).attack_range()?;
        let (min, max) = if self.env.game_mode == 1 { (r[2], r[3]) } else { (r[0], r[1]) };
        Some(pick::AttackReach { min: f64::from(min), max: f64::from(max), margin: f64::from(r[4]) })
    }

    /// Id of the item held in the main hand (empty without an item).
    pub(crate) fn held_name(&self) -> &str {
        let item = self.inventory.hotbar(self.hud.selected).and_then(|s| usize::try_from(s.item).ok());
        item.and_then(|i| self.item_names.get(i)).map_or("", String::as_str)
    }

    /// Outline of the aimed block (`targeting.md` § 4): shape (depending on the held item), boxes, corner relative
    /// to the camera origin (block's random offset included), translucent block. No
    /// outline outside the world border; in adventure, only if the held item can break
    /// the block or be placed on it.
    pub(crate) fn outline_target(&self, hit: pick::Hit) -> Option<(u16, Vec<rc_data::Aabb>, [f32; 3], bool)> {
        let pick::Hit::Block { pos, .. } = hit else { return None };
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let state = view.state(pos[0], pos[1], pos[2]);
        let (shape, shift) = aim_shape(&self.data, self.act.full_shape, self.held_name(), state, pos)?;
        let boxes = self.data.shape(shape);
        // Spectator: same test as the crosshair; adventure:
        // predicates `can_break` / `can_place_on` of the main hand's item.
        let allowed = match self.env.game_mode {
            0 | 1 => true,
            3 => self.hud.spectator_aim,
            _ => self.inventory.hotbar(self.hud.selected).is_some_and(|s| {
                let v = item_use::ItemView::new(s, &self.data, &self.act.ids);
                v.can_break(&self.data, &self.act.tags, state) || v.can_place_on(&self.data, &self.act.tags, state)
            }),
        };
        // Air has an empty shape: discarded with it.
        if self.hud.hidden || !allowed || boxes.is_empty() || !self.weather.border.block_within(pos) {
            return None;
        }
        let models = &self.tables.models;
        let translucent = models.states.get(state as usize).is_some_and(|s| {
            s.parts.iter().flat_map(|p| &p.options).any(|&(_, first, count)| {
                models.quads[first as usize..(first + count) as usize]
                    .iter()
                    .any(|q| q.layer == rc_assets::bake::Layer::Translucent)
            })
        });
        let origin = [self.shot.pos[0].floor() as i32, self.shot.pos[2].floor() as i32];
        let corner = [
            (f64::from(pos[0] - origin[0]) + shift[0]) as f32,
            (f64::from(pos[1]) + shift[1]) as f32,
            (f64::from(pos[2] - origin[1]) + shift[2]) as f32,
        ];
        Some((shape, boxes.to_vec(), corner, translucent))
    }
}
