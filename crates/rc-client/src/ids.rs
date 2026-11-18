//! Numeric identifiers of the packets used, resolved once from the official report.

use rc_data::{Bound, Phase, VersionData};

use crate::Error;

macro_rules! ids {
    ($($field:ident = $phase:ident $bound:ident $name:literal;)*) => {
        #[derive(Debug, Clone, Copy)]
        pub struct Ids { $(pub $field: i32,)* }

        impl Ids {
            pub fn resolve(d: &VersionData) -> Result<Self, Error> {
                Ok(Ids { $($field: d
                    .packet_id(Phase::$phase, Bound::$bound, concat!("minecraft:", $name))
                    .ok_or(Error::MissingPacket($name))?,)* })
            }
        }
    };
}

ids! {
    intention = Handshake ToServer "intention";
    hello = Login ToServer "hello";
    login_acknowledged = Login ToServer "login_acknowledged";
    login_compression = Login ToClient "login_compression";
    login_finished = Login ToClient "login_finished";
    login_disconnect = Login ToClient "login_disconnect";
    login_encryption_request = Login ToClient "hello";
    login_key = Login ToServer "key";

    cfg_client_information = Configuration ToServer "client_information";
    cfg_select_known_packs_out = Configuration ToServer "select_known_packs";
    cfg_keep_alive_out = Configuration ToServer "keep_alive";
    cfg_pong = Configuration ToServer "pong";
    cfg_finish_out = Configuration ToServer "finish_configuration";
    cfg_accept_code_of_conduct = Configuration ToServer "accept_code_of_conduct";
    cfg_select_known_packs = Configuration ToClient "select_known_packs";
    cfg_registry_data = Configuration ToClient "registry_data";
    cfg_keep_alive = Configuration ToClient "keep_alive";
    cfg_ping = Configuration ToClient "ping";
    cfg_finish = Configuration ToClient "finish_configuration";
    cfg_code_of_conduct = Configuration ToClient "code_of_conduct";
    cfg_disconnect = Configuration ToClient "disconnect";
    cfg_update_tags = Configuration ToClient "update_tags";
    cfg_custom_payload = Configuration ToClient "custom_payload";

    login = Play ToClient "login";
    respawn = Play ToClient "respawn";
    chunk = Play ToClient "level_chunk_with_light";
    forget_chunk = Play ToClient "forget_level_chunk";
    chunk_batch_start = Play ToClient "chunk_batch_start";
    chunk_batch_finished = Play ToClient "chunk_batch_finished";
    keep_alive = Play ToClient "keep_alive";
    ping = Play ToClient "ping";
    player_position = Play ToClient "player_position";
    chunk_center = Play ToClient "set_chunk_cache_center";
    chunk_radius = Play ToClient "set_chunk_cache_radius";
    block_update = Play ToClient "block_update";
    section_update = Play ToClient "section_blocks_update";
    block_event = Play ToClient "block_event";
    block_entity_data = Play ToClient "block_entity_data";
    start_configuration = Play ToClient "start_configuration";
    disconnect = Play ToClient "disconnect";
    update_tags = Play ToClient "update_tags";
    update_attributes = Play ToClient "update_attributes";
    abilities = Play ToClient "player_abilities";
    set_time = Play ToClient "set_time";
    light_update = Play ToClient "light_update";
    system_chat = Play ToClient "system_chat";
    player_chat = Play ToClient "player_chat";
    disguised_chat = Play ToClient "disguised_chat";
    action_bar = Play ToClient "set_action_bar_text";
    chat_out = Play ToServer "chat";
    chat_command_out = Play ToServer "chat_command";
    chat_command_signed = Play ToServer "chat_command_signed";
    chat_ack = Play ToServer "chat_ack";
    chat_session_update = Play ToServer "chat_session_update";
    command_suggestion_out = Play ToServer "command_suggestion";
    commands = Play ToClient "commands";
    command_suggestions = Play ToClient "command_suggestions";
    custom_chat_completions = Play ToClient "custom_chat_completions";
    delete_chat = Play ToClient "delete_chat";
    game_event = Play ToClient "game_event";
    set_health = Play ToClient "set_health";
    player_info_update = Play ToClient "player_info_update";
    player_info_remove = Play ToClient "player_info_remove";
    tab_list = Play ToClient "tab_list";
    boss_event = Play ToClient "boss_event";
    waypoint = Play ToClient "waypoint";
    map_item_data = Play ToClient "map_item_data";
    sound = Play ToClient "sound";
    sound_entity = Play ToClient "sound_entity";
    stop_sound = Play ToClient "stop_sound";
    level_event = Play ToClient "level_event";
    level_particles = Play ToClient "level_particles";
    set_objective = Play ToClient "set_objective";
    set_display_objective = Play ToClient "set_display_objective";
    set_score = Play ToClient "set_score";
    reset_score = Play ToClient "reset_score";
    set_player_team = Play ToClient "set_player_team";
    set_title_text = Play ToClient "set_title_text";
    set_subtitle_text = Play ToClient "set_subtitle_text";
    set_titles_animation = Play ToClient "set_titles_animation";
    clear_titles = Play ToClient "clear_titles";
    set_experience = Play ToClient "set_experience";
    set_held_slot = Play ToClient "set_held_slot";
    set_carried_item_out = Play ToServer "set_carried_item";
    bundle_item_selected_out = Play ToServer "bundle_item_selected";
    player_combat_kill = Play ToClient "player_combat_kill";
    spawn_position = Play ToClient "set_default_spawn_position";
    add_entity = Play ToClient "add_entity";
    remove_entities = Play ToClient "remove_entities";
    move_entity_pos = Play ToClient "move_entity_pos";
    move_entity_pos_rot = Play ToClient "move_entity_pos_rot";
    move_entity_rot = Play ToClient "move_entity_rot";
    teleport_entity = Play ToClient "teleport_entity";
    entity_position_sync = Play ToClient "entity_position_sync";
    rotate_head = Play ToClient "rotate_head";
    set_entity_motion = Play ToClient "set_entity_motion";
    set_entity_data = Play ToClient "set_entity_data";
    set_equipment = Play ToClient "set_equipment";
    entity_event = Play ToClient "entity_event";
    swing_animation = Play ToClient "swing_animation";
    hurt_animation = Play ToClient "hurt_animation";
    damage_event = Play ToClient "damage_event";
    set_passengers = Play ToClient "set_passengers";
    update_mob_effect = Play ToClient "update_mob_effect";
    remove_mob_effect = Play ToClient "remove_mob_effect";
    container_set_content = Play ToClient "container_set_content";
    container_set_slot = Play ToClient "container_set_slot";
    set_player_inventory = Play ToClient "set_player_inventory";
    set_cursor_item = Play ToClient "set_cursor_item";
    client_command_out = Play ToServer "client_command";
    client_information_out = Play ToServer "client_information";
    open_screen = Play ToClient "open_screen";
    container_close = Play ToClient "container_close";
    container_set_data = Play ToClient "container_set_data";
    container_close_out = Play ToServer "container_close";
    container_click_out = Play ToServer "container_click";
    set_creative_mode_slot = Play ToServer "set_creative_mode_slot";
    recipe_book_add = Play ToClient "recipe_book_add";
    recipe_book_remove = Play ToClient "recipe_book_remove";
    recipe_book_settings = Play ToClient "recipe_book_settings";
    place_ghost_recipe = Play ToClient "place_ghost_recipe";
    recipe_book_change_settings = Play ToServer "recipe_book_change_settings";
    recipe_book_seen_recipe = Play ToServer "recipe_book_seen_recipe";
    place_recipe = Play ToServer "place_recipe";
    use_item_on = Play ToServer "use_item_on";
    initialize_border = Play ToClient "initialize_border";
    set_border_center = Play ToClient "set_border_center";
    set_border_lerp_size = Play ToClient "set_border_lerp_size";
    set_border_size = Play ToClient "set_border_size";
    set_border_warning_delay = Play ToClient "set_border_warning_delay";
    set_border_warning_distance = Play ToClient "set_border_warning_distance";
    use_item = Play ToServer "use_item";
    player_action = Play ToServer "player_action";
    attack = Play ToServer "attack";
    interact = Play ToServer "interact";
    punch = Play ToServer "punch";
    spectator_action = Play ToServer "spectator_action";
    pick_item_from_block = Play ToServer "pick_item_from_block";
    pick_item_from_entity = Play ToServer "pick_item_from_entity";
    block_changed_ack = Play ToClient "block_changed_ack";
    block_destruction = Play ToClient "block_destruction";
    custom_payload = Play ToClient "custom_payload";
    pong_response = Play ToClient "pong_response";
    ticking_state = Play ToClient "ticking_state";
    ticking_step = Play ToClient "ticking_step";
    debug_sample = Play ToClient "debug_sample";
    ping_request = Play ToServer "ping_request";
    debug_subscription_request = Play ToServer "debug_subscription_request";
    change_game_mode = Play ToServer "change_game_mode";

    // Accuracy packets (`protocol-supplements.md`).
    login_cookie_request = Login ToClient "cookie_request";
    login_cookie_response = Login ToServer "cookie_response";
    login_custom_query = Login ToClient "custom_query";
    login_custom_query_answer = Login ToServer "custom_query_answer";
    cooldown = Play ToClient "cooldown";
    cfg_custom_payload_out = Configuration ToServer "custom_payload";
    cfg_cookie_request = Configuration ToClient "cookie_request";
    cfg_cookie_response = Configuration ToServer "cookie_response";
    cfg_store_cookie = Configuration ToClient "store_cookie";
    cfg_resource_pack_push = Configuration ToClient "resource_pack_push";
    cfg_resource_pack_pop = Configuration ToClient "resource_pack_pop";
    cfg_resource_pack_out = Configuration ToServer "resource_pack";
    cfg_transfer = Configuration ToClient "transfer";
    cfg_update_enabled_features = Configuration ToClient "update_enabled_features";
    cfg_reset_chat = Configuration ToClient "reset_chat";
    cfg_custom_report_details = Configuration ToClient "custom_report_details";
    cfg_server_links = Configuration ToClient "server_links";
    cfg_show_dialog = Configuration ToClient "show_dialog";
    cfg_clear_dialog = Configuration ToClient "clear_dialog";
    cfg_custom_click_action = Configuration ToServer "custom_click_action";
    bundle_delimiter = Play ToClient "bundle_delimiter";
    explode = Play ToClient "explode";
    player_rotation = Play ToClient "player_rotation";
    player_look_at = Play ToClient "player_look_at";
    set_camera = Play ToClient "set_camera";
    move_vehicle = Play ToClient "move_vehicle";
    move_vehicle_out = Play ToServer "move_vehicle";
    paddle_boat = Play ToServer "paddle_boat";
    cookie_request = Play ToClient "cookie_request";
    cookie_response = Play ToServer "cookie_response";
    store_cookie = Play ToClient "store_cookie";
    resource_pack_push = Play ToClient "resource_pack_push";
    resource_pack_pop = Play ToClient "resource_pack_pop";
    resource_pack_out = Play ToServer "resource_pack";
    transfer = Play ToClient "transfer";
    custom_report_details = Play ToClient "custom_report_details";
    server_links = Play ToClient "server_links";
    show_dialog = Play ToClient "show_dialog";
    clear_dialog = Play ToClient "clear_dialog";
    custom_click_action = Play ToServer "custom_click_action";
    chunks_biomes = Play ToClient "chunks_biomes";
    set_simulation_distance = Play ToClient "set_simulation_distance";
    change_difficulty = Play ToClient "change_difficulty";
    game_rule_values = Play ToClient "game_rule_values";
    // Isolated packets (`misc.rs`).
    set_entity_link = Play ToClient "set_entity_link";
    take_item_entity = Play ToClient "take_item_entity";
    post_effects = Play ToClient "post_effects";
    cfg_post_effects = Configuration ToClient "post_effects";
    projectile_power = Play ToClient "projectile_power";
    add_transient_block = Play ToClient "add_transient_block";
    move_minecart_along_track = Play ToClient "move_minecart_along_track";
    low_disk_space_warning = Play ToClient "low_disk_space_warning";
    server_data = Play ToClient "server_data";
    player_combat_enter = Play ToClient "player_combat_enter";
    player_combat_end = Play ToClient "player_combat_end";
    // Progress and statistics (`progress.rs`).
    update_advancements = Play ToClient "update_advancements";
    select_advancements_tab = Play ToClient "select_advancements_tab";
    award_stats = Play ToClient "award_stats";
    seen_advancements_out = Play ToServer "seen_advancements";

    chunk_batch_received = Play ToServer "chunk_batch_received";
    keep_alive_out = Play ToServer "keep_alive";
    pong = Play ToServer "pong";
    accept_teleportation = Play ToServer "accept_teleportation";
    player_loaded = Play ToServer "player_loaded";
    configuration_acknowledged = Play ToServer "configuration_acknowledged";
    move_pos = Play ToServer "move_player_pos";
    move_pos_rot = Play ToServer "move_player_pos_rot";
    move_rot = Play ToServer "move_player_rot";
    move_status = Play ToServer "move_player_status_only";
    player_input = Play ToServer "player_input";
    player_command = Play ToServer "player_command";
    client_tick_end = Play ToServer "client_tick_end";
    abilities_out = Play ToServer "player_abilities";

    // Special screens (`special-screens.md`).
    merchant_offers = Play ToClient "merchant_offers";
    update_recipes = Play ToClient "update_recipes";
    mount_screen_open = Play ToClient "mount_screen_open";
    open_book = Play ToClient "open_book";
    open_sign_editor = Play ToClient "open_sign_editor";
    select_trade = Play ToServer "select_trade";
    container_button_click = Play ToServer "container_button_click";
    container_slot_state_changed = Play ToServer "container_slot_state_changed";
    rename_item = Play ToServer "rename_item";
    set_beacon = Play ToServer "set_beacon";
    edit_book = Play ToServer "edit_book";
    sign_update = Play ToServer "sign_update";
    set_command_block = Play ToServer "set_command_block";
    set_command_minecart = Play ToServer "set_command_minecart";
    set_structure_block = Play ToServer "set_structure_block";
    set_jigsaw_block = Play ToServer "set_jigsaw_block";
    jigsaw_generate = Play ToServer "jigsaw_generate";
    set_test_block = Play ToServer "set_test_block";
    test_instance_block_action = Play ToServer "test_instance_block_action";
    teleport_to_entity = Play ToServer "teleport_to_entity";
    animate = Play ToClient "animate";

    // World options (`option-menus.md` § 5).
    change_difficulty_out = Play ToServer "change_difficulty";
    lock_difficulty_out = Play ToServer "lock_difficulty";
    set_game_rule_out = Play ToServer "set_game_rule";

    // Technical blocks and data requests (M8).
    block_entity_tag_query = Play ToServer "block_entity_tag_query";
    entity_tag_query = Play ToServer "entity_tag_query";
    tag_query = Play ToClient "tag_query";
    test_instance_block_status = Play ToClient "test_instance_block_status";
    game_test_highlight_pos = Play ToClient "game_test_highlight_pos";
}
