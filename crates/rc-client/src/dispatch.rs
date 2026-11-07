//! Which handler of the network thread reads a received packet. The choice depends on the phase
//! and the packet identifier only: no stage looks at the body before deciding.
//!
//! Each phase lists its packets in the order the network thread used to compare them, one
//! identifier after the other (cookies and resource packs, bundle delimiter, `extra.rs`,
//! `misc.rs`, `progress.rs`, then the packets of `conn.rs`); the first matching entry decides.
//! Since every identifier fits in one byte in 26.3, a dense table built once from that same list
//! answers in one index (`RUSTCRAFT_OPT_CLIENT_PACKET_TABLE`, on by default); an identifier
//! outside the table (malformed) still goes through the comparisons, so it takes the same path.

use crate::extra::{CommonPacket, ConfigExtraPacket, PlayExtraPacket};
use crate::ids::Ids;
use crate::misc::MiscPacket;

/// Login packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginPacket {
    Common(CommonPacket),
    Compression,
    Finished,
    EncryptionRequest,
    CustomQuery,
    Disconnect,
    Ignored,
}

/// Configuration packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfigurationPacket {
    Common(CommonPacket),
    Extra(ConfigExtraPacket),
    /// `post_effects` before the player exists: read and ignored (`misc.rs`).
    PostEffects,
    SelectKnownPacks,
    RegistryData,
    KeepAlive,
    Ping,
    CodeOfConduct,
    Finish,
    Tags,
    CustomPayload,
    Disconnect,
    Ignored,
}

/// Play packets, grouped by the handler that reads them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlayPacket {
    Common(CommonPacket),
    BundleDelimiter,
    Extra(PlayExtraPacket),
    Misc(MiscPacket),
    /// Progress and statistics (`progress.rs`).
    Progress,
    Session(SessionPacket),
    World(WorldPacket),
    Chat(ChatPacket),
    Hud(HudPacket),
    Container(ContainerPacket),
    Entity(EntityPacket),
    Ignored,
}

/// Life of the connection: entering a world, keep-alive, phase change, server readings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionPacket {
    Login,
    Respawn,
    KeepAlive,
    Ping,
    StartConfiguration,
    Tags,
    CustomPayload,
    PongResponse,
    DebugSample,
    Disconnect,
}

/// Chunks, blocks, light, time, border, sounds and world events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorldPacket {
    Chunk,
    ChunkBatchStart,
    ChunkBatchFinished,
    ForgetChunk,
    PlayerPosition,
    ChunkCenter,
    ChunkRadius,
    BlockUpdate,
    SectionUpdate,
    BlockEvent,
    BlockEntityData,
    Attributes,
    Time,
    LightUpdate,
    GameEvent,
    Border(BorderPacket),
    Sound,
    SoundEntity,
    LevelParticles,
    BlockChangedAck,
    BlockDestruction,
    LevelEvent,
    StopSound,
    MapData,
    SpawnPosition,
}

/// The six world border packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BorderPacket {
    Initialize,
    Center,
    LerpSize,
    Size,
    WarningDelay,
    WarningDistance,
}

/// Chat lines, commands and their suggestions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatPacket {
    SystemChat,
    ActionBar,
    PlayerChat,
    DeleteChat,
    Commands,
    CommandSuggestions,
    ChatCompletions,
    DisguisedChat,
}

/// Player list, scores, titles, bars and the player's own state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HudPacket {
    PlayerInfoUpdate,
    SetObjective,
    DisplayObjective,
    SetScore,
    ResetScore,
    PlayerTeam,
    BossEvent,
    Waypoint,
    TitleText,
    SubtitleText,
    TitlesAnimation,
    ClearTitles,
    TabList,
    PlayerInfoRemove,
    Cooldown,
    Health,
    Experience,
    CombatKill,
    HeldSlot,
    Abilities,
}

/// Inventories, recipe book, screens and the game test blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContainerPacket {
    SetContent,
    SetSlot,
    SetPlayerInventory,
    SetCursorItem,
    RecipeBookAdd,
    RecipeBookRemove,
    RecipeBookSettings,
    PlaceGhostRecipe,
    OpenScreen,
    MerchantOffers,
    UpdateRecipes,
    MountScreenOpen,
    OpenBook,
    OpenSignEditor,
    TagQuery,
    TestInstanceStatus,
    GameTestHighlight,
    Close,
    SetData,
}

/// Entity packets (`entities.md` part 1), all published as `Event::Entity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntityPacket {
    Add,
    Remove,
    MovePos,
    MovePosRot,
    MoveRot,
    Teleport,
    PositionSync,
    RotateHead,
    Motion,
    Data,
    Equipment,
    Event,
    Swing,
    Hurt,
    Damage,
    Passengers,
    Effect,
    RemoveEffect,
    Animate,
}

/// Identifiers of 26.3 all lie below this bound; the others take the comparison path.
const TABLE_SIZE: usize = 256;

/// Received packets of one phase: the comparison order, and the table built from it.
pub(crate) struct PhaseDispatch<K> {
    /// (identifier, handler) in comparison order; -1 marks a packet absent from the phase.
    order: Vec<(i32, K)>,
    /// Handler of every identifier below [`TABLE_SIZE`], `None` on the comparison path.
    table: Option<Box<[K]>>,
    unknown: K,
}

impl<K: Copy> PhaseDispatch<K> {
    fn new(order: Vec<(i32, K)>, unknown: K, use_table: bool) -> Self {
        let mut d = PhaseDispatch { order, table: None, unknown };
        if use_table {
            d.table = Some((0..TABLE_SIZE as i32).map(|id| d.compare(id)).collect());
        }
        d
    }

    /// Handler of packet `id`.
    #[inline]
    pub(crate) fn get(&self, id: i32) -> K {
        if let Some(table) = &self.table
            && let Some(&handler) = usize::try_from(id).ok().and_then(|i| table.get(i))
        {
            return handler;
        }
        self.compare(id)
    }

    /// The first entry of the comparison order whose identifier is `id`.
    pub(crate) fn compare(&self, id: i32) -> K {
        self.order.iter().find(|e| e.0 == id).map_or(self.unknown, |e| e.1)
    }

    /// Sabotage for the tests: the handlers of `a` and `b` swapped, in both paths.
    #[cfg(test)]
    pub(crate) fn swap_for_test(&mut self, a: i32, b: i32) {
        for e in &mut self.order {
            e.0 = if e.0 == a {
                b
            } else if e.0 == b {
                a
            } else {
                e.0
            };
        }
        if self.table.is_some() {
            self.table = Some((0..TABLE_SIZE as i32).map(|id| self.compare(id)).collect());
        }
    }
}

/// Handlers of the three phases read by the network thread.
pub(crate) struct Dispatch {
    pub(crate) login: PhaseDispatch<LoginPacket>,
    pub(crate) configuration: PhaseDispatch<ConfigurationPacket>,
    pub(crate) play: PhaseDispatch<PlayPacket>,
}

impl Dispatch {
    /// `use_table`: one index per packet; otherwise the comparisons in order, as before.
    pub(crate) fn new(ids: &Ids, use_table: bool) -> Self {
        Dispatch {
            login: PhaseDispatch::new(login_order(ids), LoginPacket::Ignored, use_table),
            configuration: PhaseDispatch::new(configuration_order(ids), ConfigurationPacket::Ignored, use_table),
            play: PhaseDispatch::new(play_order(ids), PlayPacket::Ignored, use_table),
        }
    }
}

/// Cookies and resource packs, in their comparison order.
fn common_order<K>(ids: [i32; 4], wrap: fn(CommonPacket) -> K) -> [(i32, K); 4] {
    use CommonPacket as C;
    let [cookie_request, store_cookie, pack_push, pack_pop] = ids;
    [
        (cookie_request, wrap(C::CookieRequest)),
        (store_cookie, wrap(C::StoreCookie)),
        (pack_push, wrap(C::PackPush)),
        (pack_pop, wrap(C::PackPop)),
    ]
}

fn login_order(i: &Ids) -> Vec<(i32, LoginPacket)> {
    use LoginPacket as L;
    // The login phase only has the cookie request (-1: the others do not exist in it).
    let mut v = common_order([i.login_cookie_request, -1, -1, -1], L::Common).to_vec();
    v.extend([
        (i.login_compression, L::Compression),
        (i.login_finished, L::Finished),
        (i.login_encryption_request, L::EncryptionRequest),
        (i.login_custom_query, L::CustomQuery),
        (i.login_disconnect, L::Disconnect),
    ]);
    v
}

fn configuration_order(i: &Ids) -> Vec<(i32, ConfigurationPacket)> {
    use ConfigExtraPacket as E;
    use ConfigurationPacket as C;
    let common = [i.cfg_cookie_request, i.cfg_store_cookie, i.cfg_resource_pack_push, i.cfg_resource_pack_pop];
    let mut v = common_order(common, C::Common).to_vec();
    v.extend([
        (i.cfg_transfer, C::Extra(E::Transfer)),
        (i.cfg_update_enabled_features, C::Extra(E::Features)),
        (i.cfg_reset_chat, C::Extra(E::ResetChat)),
        (i.cfg_custom_report_details, C::Extra(E::Report)),
        (i.cfg_server_links, C::Extra(E::Links)),
        (i.cfg_show_dialog, C::Extra(E::ShowDialog)),
        (i.cfg_clear_dialog, C::Extra(E::ClearDialog)),
        (i.cfg_post_effects, C::PostEffects),
        (i.cfg_select_known_packs, C::SelectKnownPacks),
        (i.cfg_registry_data, C::RegistryData),
        (i.cfg_keep_alive, C::KeepAlive),
        (i.cfg_ping, C::Ping),
        (i.cfg_code_of_conduct, C::CodeOfConduct),
        (i.cfg_finish, C::Finish),
        (i.cfg_update_tags, C::Tags),
        (i.cfg_custom_payload, C::CustomPayload),
        (i.cfg_disconnect, C::Disconnect),
    ]);
    v
}

fn play_order(i: &Ids) -> Vec<(i32, PlayPacket)> {
    use PlayPacket as P;
    let common = [i.cookie_request, i.store_cookie, i.resource_pack_push, i.resource_pack_pop];
    let mut v = common_order(common, P::Common).to_vec();
    v.push((i.bundle_delimiter, P::BundleDelimiter));
    v.extend(play_extra_order(i));
    v.extend(play_misc_order(i));
    v.extend([i.update_advancements, i.select_advancements_tab, i.award_stats].map(|id| (id, P::Progress)));
    v.extend(play_main_order(i));
    v
}

fn play_extra_order(i: &Ids) -> [(i32, PlayPacket); 16] {
    use PlayExtraPacket as E;
    [
        (i.ticking_state, E::TickingState),
        (i.ticking_step, E::TickingStep),
        (i.explode, E::Explode),
        (i.player_rotation, E::Rotation),
        (i.player_look_at, E::LookAt),
        (i.set_camera, E::Camera),
        (i.move_vehicle, E::Vehicle),
        (i.transfer, E::Transfer),
        (i.custom_report_details, E::Report),
        (i.server_links, E::Links),
        (i.show_dialog, E::ShowDialog),
        (i.clear_dialog, E::ClearDialog),
        (i.chunks_biomes, E::Biomes),
        (i.set_simulation_distance, E::Simulation),
        (i.change_difficulty, E::Difficulty),
        (i.game_rule_values, E::GameRules),
    ]
    .map(|(id, e)| (id, PlayPacket::Extra(e)))
}

fn play_misc_order(i: &Ids) -> [(i32, PlayPacket); 10] {
    use MiscPacket as M;
    [
        (i.set_entity_link, M::Link),
        (i.take_item_entity, M::Take),
        (i.post_effects, M::PostEffects),
        (i.projectile_power, M::ProjectilePower),
        (i.add_transient_block, M::TransientBlock),
        (i.move_minecart_along_track, M::MinecartSteps),
        (i.low_disk_space_warning, M::LowDiskSpace),
        (i.server_data, M::ServerData),
        (i.player_combat_enter, M::CombatEnter),
        (i.player_combat_end, M::CombatEnd),
    ]
    .map(|(id, m)| (id, PlayPacket::Misc(m)))
}

/// Packets read by `conn.rs` itself, in the order of its former comparison chain.
fn play_main_order(i: &Ids) -> Vec<(i32, PlayPacket)> {
    use PlayPacket::{Chat, Container, Hud, Session, World};
    use {ChatPacket as C, ContainerPacket as K, HudPacket as H, SessionPacket as S, WorldPacket as W};
    let mut v = vec![
        (i.login, Session(S::Login)),
        (i.respawn, Session(S::Respawn)),
        (i.chunk, World(W::Chunk)),
        (i.chunk_batch_start, World(W::ChunkBatchStart)),
        (i.chunk_batch_finished, World(W::ChunkBatchFinished)),
        (i.forget_chunk, World(W::ForgetChunk)),
        (i.keep_alive, Session(S::KeepAlive)),
        (i.ping, Session(S::Ping)),
        (i.player_position, World(W::PlayerPosition)),
        (i.chunk_center, World(W::ChunkCenter)),
        (i.chunk_radius, World(W::ChunkRadius)),
        (i.block_update, World(W::BlockUpdate)),
        (i.section_update, World(W::SectionUpdate)),
        (i.block_event, World(W::BlockEvent)),
        (i.block_entity_data, World(W::BlockEntityData)),
        (i.start_configuration, Session(S::StartConfiguration)),
        (i.update_tags, Session(S::Tags)),
        (i.update_attributes, World(W::Attributes)),
        (i.set_time, World(W::Time)),
        (i.system_chat, Chat(C::SystemChat)),
        (i.action_bar, Chat(C::ActionBar)),
        (i.player_chat, Chat(C::PlayerChat)),
        (i.delete_chat, Chat(C::DeleteChat)),
        (i.commands, Chat(C::Commands)),
        (i.command_suggestions, Chat(C::CommandSuggestions)),
        (i.custom_chat_completions, Chat(C::ChatCompletions)),
        (i.disguised_chat, Chat(C::DisguisedChat)),
        (i.light_update, World(W::LightUpdate)),
        (i.game_event, World(W::GameEvent)),
    ];
    v.extend(border_order(i));
    v.extend([
        (i.player_info_update, Hud(H::PlayerInfoUpdate)),
        (i.set_objective, Hud(H::SetObjective)),
        (i.set_display_objective, Hud(H::DisplayObjective)),
        (i.set_score, Hud(H::SetScore)),
        (i.reset_score, Hud(H::ResetScore)),
        (i.set_player_team, Hud(H::PlayerTeam)),
        (i.sound, World(W::Sound)),
        (i.sound_entity, World(W::SoundEntity)),
        (i.level_particles, World(W::LevelParticles)),
        (i.block_changed_ack, World(W::BlockChangedAck)),
        (i.block_destruction, World(W::BlockDestruction)),
        (i.level_event, World(W::LevelEvent)),
        (i.stop_sound, World(W::StopSound)),
        (i.boss_event, Hud(H::BossEvent)),
        (i.waypoint, Hud(H::Waypoint)),
        (i.map_item_data, World(W::MapData)),
        (i.set_title_text, Hud(H::TitleText)),
        (i.set_subtitle_text, Hud(H::SubtitleText)),
        (i.set_titles_animation, Hud(H::TitlesAnimation)),
        (i.clear_titles, Hud(H::ClearTitles)),
        (i.tab_list, Hud(H::TabList)),
        (i.player_info_remove, Hud(H::PlayerInfoRemove)),
        (i.cooldown, Hud(H::Cooldown)),
        (i.set_health, Hud(H::Health)),
        (i.set_experience, Hud(H::Experience)),
        (i.container_set_content, Container(K::SetContent)),
        (i.container_set_slot, Container(K::SetSlot)),
        (i.set_player_inventory, Container(K::SetPlayerInventory)),
        (i.set_cursor_item, Container(K::SetCursorItem)),
        (i.recipe_book_add, Container(K::RecipeBookAdd)),
        (i.recipe_book_remove, Container(K::RecipeBookRemove)),
        (i.recipe_book_settings, Container(K::RecipeBookSettings)),
        (i.place_ghost_recipe, Container(K::PlaceGhostRecipe)),
        (i.open_screen, Container(K::OpenScreen)),
        (i.merchant_offers, Container(K::MerchantOffers)),
        (i.update_recipes, Container(K::UpdateRecipes)),
        (i.mount_screen_open, Container(K::MountScreenOpen)),
        (i.open_book, Container(K::OpenBook)),
        (i.open_sign_editor, Container(K::OpenSignEditor)),
        (i.tag_query, Container(K::TagQuery)),
        (i.test_instance_block_status, Container(K::TestInstanceStatus)),
        (i.game_test_highlight_pos, Container(K::GameTestHighlight)),
        (i.container_close, Container(K::Close)),
        (i.container_set_data, Container(K::SetData)),
    ]);
    v.extend(entity_order(i));
    // `ticking_state` and `ticking_step` came next: `extra.rs` always read them first.
    v.extend([
        (i.spawn_position, World(W::SpawnPosition)),
        (i.player_combat_kill, Hud(H::CombatKill)),
        (i.set_held_slot, Hud(H::HeldSlot)),
        (i.abilities, Hud(H::Abilities)),
        (i.custom_payload, Session(S::CustomPayload)),
        (i.pong_response, Session(S::PongResponse)),
        (i.debug_sample, Session(S::DebugSample)),
        (i.disconnect, Session(S::Disconnect)),
    ]);
    v
}

fn border_order(i: &Ids) -> [(i32, PlayPacket); 6] {
    use BorderPacket as B;
    [
        (i.initialize_border, B::Initialize),
        (i.set_border_center, B::Center),
        (i.set_border_lerp_size, B::LerpSize),
        (i.set_border_size, B::Size),
        (i.set_border_warning_delay, B::WarningDelay),
        (i.set_border_warning_distance, B::WarningDistance),
    ]
    .map(|(id, b)| (id, PlayPacket::World(WorldPacket::Border(b))))
}

fn entity_order(i: &Ids) -> [(i32, PlayPacket); 19] {
    use EntityPacket as E;
    [
        (i.add_entity, E::Add),
        (i.remove_entities, E::Remove),
        (i.move_entity_pos, E::MovePos),
        (i.move_entity_pos_rot, E::MovePosRot),
        (i.move_entity_rot, E::MoveRot),
        (i.teleport_entity, E::Teleport),
        (i.entity_position_sync, E::PositionSync),
        (i.rotate_head, E::RotateHead),
        (i.set_entity_motion, E::Motion),
        (i.set_entity_data, E::Data),
        (i.set_equipment, E::Equipment),
        (i.entity_event, E::Event),
        (i.swing_animation, E::Swing),
        (i.hurt_animation, E::Hurt),
        (i.damage_event, E::Damage),
        (i.set_passengers, E::Passengers),
        (i.update_mob_effect, E::Effect),
        (i.remove_mob_effect, E::RemoveEffect),
        (i.animate, E::Animate),
    ]
    .map(|(id, e)| (id, PlayPacket::Entity(e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Identifiers checked: every byte value, negative ones, and a few beyond the table.
    fn probe_ids() -> impl Iterator<Item = i32> {
        (-3..300).chain([i32::MIN, -1_000_000, 1_000_000, i32::MAX])
    }

    fn table_matches_comparisons<K: Copy + PartialEq + std::fmt::Debug>(name: &str, d: &PhaseDispatch<K>) {
        for id in probe_ids() {
            assert_eq!(d.get(id), d.compare(id), "{name}: identifier {id}");
        }
    }

    #[test]
    fn table_gives_the_handler_of_the_comparisons_for_every_identifier() {
        let Some(data) = rc_testdata::version_data() else { return };
        let ids = Ids::resolve(&data).expect("packet identifiers");
        let d = Dispatch::new(&ids, true);
        table_matches_comparisons("login", &d.login);
        table_matches_comparisons("configuration", &d.configuration);
        table_matches_comparisons("play", &d.play);
        // Every valid identifier is covered by the table.
        let max = data.packets.iter().filter(|p| p.bound == rc_data::Bound::ToClient).map(|p| p.id).max();
        assert!(max.is_some_and(|m| (0..TABLE_SIZE as i32).contains(&m)), "largest identifier {max:?}");
        // A sample of the decisions, so that a shift of the whole list is seen too.
        assert_eq!(d.play.get(ids.move_entity_pos_rot), PlayPacket::Entity(EntityPacket::MovePosRot));
        assert_eq!(d.play.get(ids.ticking_state), PlayPacket::Extra(PlayExtraPacket::TickingState));
        assert_eq!(d.play.get(ids.update_advancements), PlayPacket::Progress);
        assert_eq!(d.configuration.get(ids.cfg_post_effects), ConfigurationPacket::PostEffects);
        assert_eq!(d.login.get(-1), LoginPacket::Common(CommonPacket::StoreCookie));
    }

    #[test]
    fn valid_identifiers_are_never_claimed_twice_in_a_phase() {
        let Some(data) = rc_testdata::version_data() else { return };
        let ids = Ids::resolve(&data).expect("packet identifiers");
        fn check<K: std::fmt::Debug>(name: &str, order: &[(i32, K)]) {
            let mut seen = hashbrown::HashMap::new();
            for (id, k) in order.iter().filter(|e| e.0 >= 0) {
                if let Some(first) = seen.insert(*id, k) {
                    panic!("{name}: identifier {id} claimed by {first:?} then {k:?}");
                }
            }
        }
        check("login", &login_order(&ids));
        check("configuration", &configuration_order(&ids));
        check("play", &play_order(&ids));
    }

    /// Micro-bench (loaded machine figures in `docs/perf.md`):
    /// `cargo test --release -p rc-client packet_dispatch_bench -- --ignored --nocapture`.
    /// The play identifiers of the reference capture, in order, decided by the comparisons then
    /// by the table; then one such stream per thread, from 1 thread to every logical core.
    #[test]
    #[ignore]
    fn packet_dispatch_bench() {
        use std::hint::black_box;
        let (Some(data), Some(capture)) = (rc_testdata::version_data(), rc_testdata::reference_capture()) else {
            return;
        };
        let ids = Ids::resolve(&data).expect("packet identifiers");
        let stream: Vec<i32> = rc_testdata::records(&capture)
            .iter()
            .filter(|r| !r.to_server && r.phase == 4)
            .map(|r| rc_proto::varint::read_varint(&mut rc_proto::Reader::new(r.packet)).expect("identifier"))
            .collect();
        const ROUNDS: usize = 200;
        let decide_all = |d: &PhaseDispatch<PlayPacket>, table: bool| {
            for _ in 0..ROUNDS {
                for &id in &stream {
                    let id = black_box(id);
                    black_box(if table { d.get(id) } else { d.compare(id) });
                }
            }
        };
        let d = Dispatch::new(&ids, true);
        let packets = (ROUNDS * stream.len()) as f64;
        for trial in 0..3 {
            for (name, table) in [("comparisons", false), ("table", true)] {
                let t = std::time::Instant::now();
                decide_all(&d.play, table);
                eprintln!("trial {trial}, 1 core: {name} {:.2} ns per packet", t.elapsed().as_nanos() as f64 / packets);
            }
        }
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let mut threads = 1;
        while threads <= cores {
            for (name, table) in [("comparisons", false), ("table", true)] {
                let t = std::time::Instant::now();
                std::thread::scope(|s| {
                    for _ in 0..threads {
                        s.spawn(|| decide_all(&Dispatch::new(&ids, true).play, table));
                    }
                });
                let rate = packets * threads as f64 / t.elapsed().as_secs_f64() / 1e6;
                eprintln!("{threads} threads: {name} {rate:.0} M packets/s in all");
            }
            threads = if threads == cores { cores + 1 } else { (threads * 2).min(cores) };
        }
    }

    #[test]
    fn handlers_stay_small() {
        // The play table holds 256 of these: one or two bytes each.
        assert!(size_of::<PlayPacket>() <= 2, "{}", size_of::<PlayPacket>());
        assert!(size_of::<ConfigurationPacket>() <= 2);
        assert!(size_of::<LoginPacket>() <= 2);
    }
}
