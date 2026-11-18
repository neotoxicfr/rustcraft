//! Packets the reference capture lacks, played after its last packet: the common packets, a
//! bundle, unknown identifiers, a few packets of every handler group, a configuration round trip,
//! then the server's disconnection. Each entry: (capture phase number, identifier then body).

use rc_data::{Bound, Phase, VersionData};
use rc_proto::Encode;
use rc_proto::varint::write_varint;

struct Tail<'a> {
    data: &'a VersionData,
    out: Vec<(u8, Vec<u8>)>,
}

impl Tail<'_> {
    fn raw(&mut self, phase: u8, id: i32, body: impl FnOnce(&mut Vec<u8>)) {
        let mut w = Vec::new();
        write_varint(&mut w, id);
        body(&mut w);
        self.out.push((phase, w));
    }

    fn named(&mut self, phase: Phase, name: &str, body: impl FnOnce(&mut Vec<u8>)) {
        let id = self.data.packet_id(phase, Bound::ToClient, &format!("minecraft:{name}")).expect(name);
        let number = if phase == Phase::Play { 4 } else { 3 };
        self.raw(number, id, body);
    }

    fn play(&mut self, name: &str, body: impl FnOnce(&mut Vec<u8>)) {
        self.named(Phase::Play, name, body);
    }

    fn config(&mut self, name: &str, body: impl FnOnce(&mut Vec<u8>)) {
        self.named(Phase::Configuration, name, body);
    }
}

/// Text as a nameless network NBT string tag.
fn nbt_text(w: &mut Vec<u8>, s: &str) {
    w.push(8);
    w.extend_from_slice(&(s.len() as u16).to_be_bytes());
    w.extend_from_slice(s.as_bytes());
}

pub(super) fn synthetic_tail(data: &VersionData) -> Vec<(u8, Vec<u8>)> {
    let mut t = Tail { data, out: Vec::new() };
    common_packets(&mut t);
    // A bundle of two packets.
    t.play("bundle_delimiter", |_| {});
    t.play("set_held_slot", |w| write_varint(w, 3));
    t.play("animate", |w| {
        write_varint(w, 7);
        1u8.encode(w);
    });
    t.play("bundle_delimiter", |_| {});
    // Unknown identifiers: inside the table, beyond it, negative.
    for id in [0xF0, 300, -1] {
        t.raw(4, id, |w| w.push(0));
    }
    hud_and_world_packets(&mut t);
    extra_and_misc_packets(&mut t);
    entity_and_chat_packets(&mut t);
    configuration_round_trip(&mut t);
    t.play("disconnect", |w| nbt_text(w, "bye"));
    t.out
}

fn common_packets(t: &mut Tail<'_>) {
    t.play("cookie_request", |w| rc_proto::write_str(w, "rc:k"));
    t.play("store_cookie", |w| {
        rc_proto::write_str(w, "rc:k");
        write_varint(w, 3);
        w.extend_from_slice(&[1, 2, 3]);
    });
    t.play("cookie_request", |w| rc_proto::write_str(w, "rc:k"));
    t.play("resource_pack_push", |w| {
        1u128.encode(w);
        rc_proto::write_str(w, "ftp://bad");
        rc_proto::write_str(w, "");
        false.encode(w);
        false.encode(w);
    });
    t.play("resource_pack_push", |w| {
        2u128.encode(w);
        rc_proto::write_str(w, "https://example.com/p.zip");
        rc_proto::write_str(w, "");
        true.encode(w);
        true.encode(w);
        nbt_text(w, "Please");
    });
    t.play("resource_pack_pop", |w| {
        true.encode(w);
        2u128.encode(w);
    });
}

fn hud_and_world_packets(t: &mut Tail<'_>) {
    t.play("set_chunk_cache_radius", |w| write_varint(w, 8));
    t.play("set_chunk_cache_center", |w| {
        write_varint(w, 1);
        write_varint(w, -2);
    });
    t.play("block_changed_ack", |w| write_varint(w, 42));
    t.play("open_book", |w| write_varint(w, 0));
    t.play("container_close", |w| write_varint(w, 1));
    t.play("cooldown", |w| {
        rc_proto::write_str(w, "ender_pearl");
        write_varint(w, 20);
    });
    t.play("cooldown", |w| {
        rc_proto::write_str(w, "rc:x");
        write_varint(w, 5);
    });
    t.play("set_health", |w| {
        20.0f32.encode(w);
        write_varint(w, 18);
        4.5f32.encode(w);
    });
    t.play("set_experience", |w| {
        0.5f32.encode(w);
        write_varint(w, 3);
        write_varint(w, 30);
    });
    t.play("clear_titles", |w| true.encode(w));
    t.play("set_title_text", |w| nbt_text(w, "Title"));
    t.play("set_subtitle_text", |w| nbt_text(w, "Sub"));
    t.play("set_titles_animation", |w| [10i32, 70, 20].iter().for_each(|v| v.encode(w)));
    t.play("game_event", |w| {
        3u8.encode(w);
        1.0f32.encode(w);
    });
    t.play("ping", |w| 77i32.encode(w));
    t.play("keep_alive", |w| 99i64.encode(w));
}

fn extra_and_misc_packets(t: &mut Tail<'_>) {
    t.play("set_simulation_distance", |w| write_varint(w, 10));
    t.play("set_camera", |w| write_varint(w, 5));
    t.play("clear_dialog", |_| {});
    for (difficulty, locked) in [(2, true), (9, false)] {
        t.play("change_difficulty", |w| {
            write_varint(w, difficulty);
            locked.encode(w);
        });
    }
    t.play("player_rotation", |w| {
        90.0f32.encode(w);
        false.encode(w);
        10.0f32.encode(w);
        true.encode(w);
    });
    t.play("take_item_entity", |w| [1, 2, 3].iter().for_each(|&v| write_varint(w, v)));
    t.play("low_disk_space_warning", |_| {});
    t.play("player_combat_enter", |_| {});
    t.play("player_combat_end", |w| write_varint(w, 40));
    t.play("set_entity_link", |w| {
        1i32.encode(w);
        2i32.encode(w);
    });
    t.play("projectile_power", |w| {
        write_varint(w, 1);
        0.1f64.encode(w);
    });
}

fn entity_and_chat_packets(t: &mut Tail<'_>) {
    t.play("rotate_head", |w| {
        write_varint(w, 5);
        64i8.encode(w);
    });
    t.play("hurt_animation", |w| {
        write_varint(w, 5);
        30.0f32.encode(w);
    });
    t.play("remove_mob_effect", |w| {
        write_varint(w, 5);
        write_varint(w, 1);
    });
    t.play("entity_event", |w| {
        5i32.encode(w);
        2i8.encode(w);
    });
    t.play("system_chat", |w| {
        nbt_text(w, "hello");
        false.encode(w);
    });
    t.play("system_chat", |w| {
        nbt_text(w, "<Bob> hi");
        true.encode(w);
    });
    t.play("set_action_bar_text", |w| nbt_text(w, "bar"));
}

fn configuration_round_trip(t: &mut Tail<'_>) {
    t.play("start_configuration", |_| {});
    t.config("keep_alive", |w| 5i64.encode(w));
    t.config("ping", |w| 6i32.encode(w));
    t.config("reset_chat", |_| {});
    t.config("update_enabled_features", |w| {
        write_varint(w, 1);
        rc_proto::write_str(w, "minecraft:vanilla");
    });
    t.config("custom_payload", |w| {
        rc_proto::write_str(w, "minecraft:brand");
        rc_proto::write_str(w, "test");
    });
    t.config("clear_dialog", |_| {});
    t.config("cookie_request", |w| rc_proto::write_str(w, "rc:k"));
    t.config("store_cookie", |w| {
        rc_proto::write_str(w, "rc:c");
        write_varint(w, 0);
    });
    t.config("resource_pack_pop", |w| false.encode(w));
    t.raw(3, 0xF0, |w| w.push(0));
    t.config("finish_configuration", |_| {});
}
