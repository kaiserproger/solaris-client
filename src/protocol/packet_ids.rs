//! Minecraft Java 26.3 protocol 777 packet identifiers.
//! Keep wire IDs here; the report-backed test checks the same constants used by senders/dispatch.

macro_rules! packets {
    ($($name:ident = $id:literal => $wire:literal),* $(,)?) => {
        $(pub const $name: i32 = $id;)*
        #[cfg(test)]
        pub(crate) const IDS: &[(&str, i32)] = &[$(($wire, $name)),*];
    };
}

pub mod login {
    pub mod clientbound {
        packets! {
            LOGIN_DISCONNECT = 0 => "minecraft:login_disconnect",
            HELLO = 1 => "minecraft:hello",
            LOGIN_FINISHED = 2 => "minecraft:login_finished",
            LOGIN_COMPRESSION = 3 => "minecraft:login_compression",
            CUSTOM_QUERY = 4 => "minecraft:custom_query",
            COOKIE_REQUEST = 5 => "minecraft:cookie_request",
        }
    }
    pub mod serverbound {
        packets! {
            HELLO = 0 => "minecraft:hello",
            KEY = 1 => "minecraft:key",
            CUSTOM_QUERY_ANSWER = 2 => "minecraft:custom_query_answer",
            LOGIN_ACKNOWLEDGED = 3 => "minecraft:login_acknowledged",
            COOKIE_RESPONSE = 4 => "minecraft:cookie_response",
        }
    }
}

pub mod configuration {
    pub mod clientbound {
        packets! {
            COOKIE_REQUEST = 0 => "minecraft:cookie_request",
            CUSTOM_PAYLOAD = 1 => "minecraft:custom_payload",
            DISCONNECT = 2 => "minecraft:disconnect",
            FINISH_CONFIGURATION = 3 => "minecraft:finish_configuration",
            KEEP_ALIVE = 4 => "minecraft:keep_alive",
            PING = 5 => "minecraft:ping",
            RESET_CHAT = 6 => "minecraft:reset_chat",
            REGISTRY_DATA = 7 => "minecraft:registry_data",
            RESOURCE_PACK_POP = 8 => "minecraft:resource_pack_pop",
            RESOURCE_PACK_PUSH = 9 => "minecraft:resource_pack_push",
            POST_EFFECTS = 10 => "minecraft:post_effects",
            STORE_COOKIE = 11 => "minecraft:store_cookie",
            TRANSFER = 12 => "minecraft:transfer",
            UPDATE_ENABLED_FEATURES = 13 => "minecraft:update_enabled_features",
            UPDATE_TAGS = 14 => "minecraft:update_tags",
            SELECT_KNOWN_PACKS = 15 => "minecraft:select_known_packs",
            CUSTOM_REPORT_DETAILS = 16 => "minecraft:custom_report_details",
            SERVER_LINKS = 17 => "minecraft:server_links",
            CLEAR_DIALOG = 18 => "minecraft:clear_dialog",
            SHOW_DIALOG = 19 => "minecraft:show_dialog",
            CODE_OF_CONDUCT = 20 => "minecraft:code_of_conduct",
        }
    }
    pub mod serverbound {
        packets! {
            CLIENT_INFORMATION = 0 => "minecraft:client_information",
            COOKIE_RESPONSE = 1 => "minecraft:cookie_response",
            CUSTOM_PAYLOAD = 2 => "minecraft:custom_payload",
            FINISH_CONFIGURATION = 3 => "minecraft:finish_configuration",
            KEEP_ALIVE = 4 => "minecraft:keep_alive",
            PONG = 5 => "minecraft:pong",
            RESOURCE_PACK = 6 => "minecraft:resource_pack",
            SELECT_KNOWN_PACKS = 7 => "minecraft:select_known_packs",
            CUSTOM_CLICK_ACTION = 8 => "minecraft:custom_click_action",
            ACCEPT_CODE_OF_CONDUCT = 9 => "minecraft:accept_code_of_conduct",
        }
    }
}

pub mod play {
    pub mod clientbound {
        packets! {
            BUNDLE_DELIMITER = 0 => "minecraft:bundle_delimiter",
            ADD_ENTITY = 1 => "minecraft:add_entity",
            ANIMATE = 2 => "minecraft:animate",
            AWARD_STATS = 3 => "minecraft:award_stats",
            BLOCK_CHANGED_ACK = 4 => "minecraft:block_changed_ack",
            BLOCK_DESTRUCTION = 5 => "minecraft:block_destruction",
            BLOCK_ENTITY_DATA = 6 => "minecraft:block_entity_data",
            BLOCK_EVENT = 7 => "minecraft:block_event",
            BLOCK_UPDATE = 8 => "minecraft:block_update",
            BOSS_EVENT = 9 => "minecraft:boss_event",
            CHANGE_DIFFICULTY = 10 => "minecraft:change_difficulty",
            CHUNK_BATCH_FINISHED = 11 => "minecraft:chunk_batch_finished",
            CHUNK_BATCH_START = 12 => "minecraft:chunk_batch_start",
            CHUNKS_BIOMES = 13 => "minecraft:chunks_biomes",
            CLEAR_TITLES = 14 => "minecraft:clear_titles",
            COMMAND_SUGGESTIONS = 15 => "minecraft:command_suggestions",
            COMMANDS = 16 => "minecraft:commands",
            CONTAINER_CLOSE = 17 => "minecraft:container_close",
            CONTAINER_SET_CONTENT = 18 => "minecraft:container_set_content",
            CONTAINER_SET_DATA = 19 => "minecraft:container_set_data",
            CONTAINER_SET_SLOT = 20 => "minecraft:container_set_slot",
            COOKIE_REQUEST = 21 => "minecraft:cookie_request",
            COOLDOWN = 22 => "minecraft:cooldown",
            CUSTOM_CHAT_COMPLETIONS = 23 => "minecraft:custom_chat_completions",
            CUSTOM_PAYLOAD = 24 => "minecraft:custom_payload",
            DAMAGE_EVENT = 25 => "minecraft:damage_event",
            DEBUG_BLOCK_VALUE = 26 => "minecraft:debug/block_value",
            DEBUG_CHUNK_VALUE = 27 => "minecraft:debug/chunk_value",
            DEBUG_ENTITY_VALUE = 28 => "minecraft:debug/entity_value",
            DEBUG_EVENT = 29 => "minecraft:debug/event",
            DEBUG_SAMPLE = 30 => "minecraft:debug_sample",
            DELETE_CHAT = 31 => "minecraft:delete_chat",
            DISCONNECT = 32 => "minecraft:disconnect",
            DISGUISED_CHAT = 33 => "minecraft:disguised_chat",
            ENTITY_EVENT = 34 => "minecraft:entity_event",
            ENTITY_POSITION_SYNC = 35 => "minecraft:entity_position_sync",
            EXPLODE = 36 => "minecraft:explode",
            ADD_TRANSIENT_BLOCK = 37 => "minecraft:add_transient_block",
            FORGET_LEVEL_CHUNK = 38 => "minecraft:forget_level_chunk",
            GAME_EVENT = 39 => "minecraft:game_event",
            GAME_RULE_VALUES = 40 => "minecraft:game_rule_values",
            GAME_TEST_HIGHLIGHT_POS = 41 => "minecraft:game_test_highlight_pos",
            MOUNT_SCREEN_OPEN = 42 => "minecraft:mount_screen_open",
            HURT_ANIMATION = 43 => "minecraft:hurt_animation",
            INITIALIZE_BORDER = 44 => "minecraft:initialize_border",
            KEEP_ALIVE = 45 => "minecraft:keep_alive",
            LEVEL_CHUNK_WITH_LIGHT = 46 => "minecraft:level_chunk_with_light",
            LEVEL_EVENT = 47 => "minecraft:level_event",
            LEVEL_PARTICLES = 48 => "minecraft:level_particles",
            LIGHT_UPDATE = 49 => "minecraft:light_update",
            LOGIN = 50 => "minecraft:login",
            LOW_DISK_SPACE_WARNING = 51 => "minecraft:low_disk_space_warning",
            MAP_ITEM_DATA = 52 => "minecraft:map_item_data",
            MERCHANT_OFFERS = 53 => "minecraft:merchant_offers",
            MOVE_ENTITY_POS = 54 => "minecraft:move_entity_pos",
            MOVE_ENTITY_POS_ROT = 55 => "minecraft:move_entity_pos_rot",
            MOVE_MINECART_ALONG_TRACK = 56 => "minecraft:move_minecart_along_track",
            MOVE_ENTITY_ROT = 57 => "minecraft:move_entity_rot",
            MOVE_VEHICLE = 58 => "minecraft:move_vehicle",
            OPEN_BOOK = 59 => "minecraft:open_book",
            OPEN_SCREEN = 60 => "minecraft:open_screen",
            OPEN_SIGN_EDITOR = 61 => "minecraft:open_sign_editor",
            PING = 62 => "minecraft:ping",
            PONG_RESPONSE = 63 => "minecraft:pong_response",
            PLACE_GHOST_RECIPE = 64 => "minecraft:place_ghost_recipe",
            PLAYER_ABILITIES = 65 => "minecraft:player_abilities",
            PLAYER_CHAT = 66 => "minecraft:player_chat",
            PLAYER_COMBAT_END = 67 => "minecraft:player_combat_end",
            PLAYER_COMBAT_ENTER = 68 => "minecraft:player_combat_enter",
            PLAYER_COMBAT_KILL = 69 => "minecraft:player_combat_kill",
            PLAYER_INFO_REMOVE = 70 => "minecraft:player_info_remove",
            PLAYER_INFO_UPDATE = 71 => "minecraft:player_info_update",
            PLAYER_LOOK_AT = 72 => "minecraft:player_look_at",
            PLAYER_POSITION = 73 => "minecraft:player_position",
            PLAYER_ROTATION = 74 => "minecraft:player_rotation",
            RECIPE_BOOK_ADD = 75 => "minecraft:recipe_book_add",
            RECIPE_BOOK_REMOVE = 76 => "minecraft:recipe_book_remove",
            RECIPE_BOOK_SETTINGS = 77 => "minecraft:recipe_book_settings",
            REMOVE_ENTITIES = 78 => "minecraft:remove_entities",
            REMOVE_MOB_EFFECT = 79 => "minecraft:remove_mob_effect",
            RESET_SCORE = 80 => "minecraft:reset_score",
            RESOURCE_PACK_POP = 81 => "minecraft:resource_pack_pop",
            RESOURCE_PACK_PUSH = 82 => "minecraft:resource_pack_push",
            POST_EFFECTS = 83 => "minecraft:post_effects",
            RESPAWN = 84 => "minecraft:respawn",
            ROTATE_HEAD = 85 => "minecraft:rotate_head",
            SECTION_BLOCKS_UPDATE = 86 => "minecraft:section_blocks_update",
            SELECT_ADVANCEMENTS_TAB = 87 => "minecraft:select_advancements_tab",
            SERVER_DATA = 88 => "minecraft:server_data",
            SET_ACTION_BAR_TEXT = 89 => "minecraft:set_action_bar_text",
            SET_BORDER_CENTER = 90 => "minecraft:set_border_center",
            SET_BORDER_LERP_SIZE = 91 => "minecraft:set_border_lerp_size",
            SET_BORDER_SIZE = 92 => "minecraft:set_border_size",
            SET_BORDER_WARNING_DELAY = 93 => "minecraft:set_border_warning_delay",
            SET_BORDER_WARNING_DISTANCE = 94 => "minecraft:set_border_warning_distance",
            SET_CAMERA = 95 => "minecraft:set_camera",
            SET_CHUNK_CACHE_CENTER = 96 => "minecraft:set_chunk_cache_center",
            SET_CHUNK_CACHE_RADIUS = 97 => "minecraft:set_chunk_cache_radius",
            SET_CURSOR_ITEM = 98 => "minecraft:set_cursor_item",
            SET_DEFAULT_SPAWN_POSITION = 99 => "minecraft:set_default_spawn_position",
            SET_DISPLAY_OBJECTIVE = 100 => "minecraft:set_display_objective",
            SET_ENTITY_DATA = 101 => "minecraft:set_entity_data",
            SET_ENTITY_LINK = 102 => "minecraft:set_entity_link",
            SET_ENTITY_MOTION = 103 => "minecraft:set_entity_motion",
            SET_EQUIPMENT = 104 => "minecraft:set_equipment",
            SET_EXPERIENCE = 105 => "minecraft:set_experience",
            SET_HEALTH = 106 => "minecraft:set_health",
            SET_HELD_SLOT = 107 => "minecraft:set_held_slot",
            SET_OBJECTIVE = 108 => "minecraft:set_objective",
            SET_PASSENGERS = 109 => "minecraft:set_passengers",
            SET_PLAYER_INVENTORY = 110 => "minecraft:set_player_inventory",
            SET_PLAYER_TEAM = 111 => "minecraft:set_player_team",
            SET_SCORE = 112 => "minecraft:set_score",
            SET_SIMULATION_DISTANCE = 113 => "minecraft:set_simulation_distance",
            SET_SUBTITLE_TEXT = 114 => "minecraft:set_subtitle_text",
            SET_TIME = 115 => "minecraft:set_time",
            SET_TITLE_TEXT = 116 => "minecraft:set_title_text",
            SET_TITLES_ANIMATION = 117 => "minecraft:set_titles_animation",
            SOUND_ENTITY = 118 => "minecraft:sound_entity",
            SOUND = 119 => "minecraft:sound",
            START_CONFIGURATION = 120 => "minecraft:start_configuration",
            STOP_SOUND = 121 => "minecraft:stop_sound",
            STORE_COOKIE = 122 => "minecraft:store_cookie",
            SWING_ANIMATION = 123 => "minecraft:swing_animation",
            SYSTEM_CHAT = 124 => "minecraft:system_chat",
            TAB_LIST = 125 => "minecraft:tab_list",
            TAG_QUERY = 126 => "minecraft:tag_query",
            TAKE_ITEM_ENTITY = 127 => "minecraft:take_item_entity",
            TELEPORT_ENTITY = 128 => "minecraft:teleport_entity",
            TEST_INSTANCE_BLOCK_STATUS = 129 => "minecraft:test_instance_block_status",
            TICKING_STATE = 130 => "minecraft:ticking_state",
            TICKING_STEP = 131 => "minecraft:ticking_step",
            TRANSFER = 132 => "minecraft:transfer",
            UPDATE_ADVANCEMENTS = 133 => "minecraft:update_advancements",
            UPDATE_ATTRIBUTES = 134 => "minecraft:update_attributes",
            UPDATE_MOB_EFFECT = 135 => "minecraft:update_mob_effect",
            UPDATE_RECIPES = 136 => "minecraft:update_recipes",
            UPDATE_TAGS = 137 => "minecraft:update_tags",
            PROJECTILE_POWER = 138 => "minecraft:projectile_power",
            CUSTOM_REPORT_DETAILS = 139 => "minecraft:custom_report_details",
            SERVER_LINKS = 140 => "minecraft:server_links",
            WAYPOINT = 141 => "minecraft:waypoint",
            CLEAR_DIALOG = 142 => "minecraft:clear_dialog",
            SHOW_DIALOG = 143 => "minecraft:show_dialog",
        }
    }
    pub mod serverbound {
        packets! {
            ACCEPT_TELEPORTATION = 0 => "minecraft:accept_teleportation",
            ATTACK = 1 => "minecraft:attack",
            BLOCK_ENTITY_TAG_QUERY = 2 => "minecraft:block_entity_tag_query",
            BUNDLE_ITEM_SELECTED = 3 => "minecraft:bundle_item_selected",
            CHANGE_DIFFICULTY = 4 => "minecraft:change_difficulty",
            CHANGE_GAME_MODE = 5 => "minecraft:change_game_mode",
            CHAT_ACK = 6 => "minecraft:chat_ack",
            CHAT_COMMAND = 7 => "minecraft:chat_command",
            CHAT_COMMAND_SIGNED = 8 => "minecraft:chat_command_signed",
            CHAT = 9 => "minecraft:chat",
            CHAT_SESSION_UPDATE = 10 => "minecraft:chat_session_update",
            CHUNK_BATCH_RECEIVED = 11 => "minecraft:chunk_batch_received",
            CLIENT_COMMAND = 12 => "minecraft:client_command",
            CLIENT_TICK_END = 13 => "minecraft:client_tick_end",
            CLIENT_INFORMATION = 14 => "minecraft:client_information",
            COMMAND_SUGGESTION = 15 => "minecraft:command_suggestion",
            CONFIGURATION_ACKNOWLEDGED = 16 => "minecraft:configuration_acknowledged",
            CONTAINER_BUTTON_CLICK = 17 => "minecraft:container_button_click",
            CONTAINER_CLICK = 18 => "minecraft:container_click",
            CONTAINER_CLOSE = 19 => "minecraft:container_close",
            CONTAINER_SLOT_STATE_CHANGED = 20 => "minecraft:container_slot_state_changed",
            COOKIE_RESPONSE = 21 => "minecraft:cookie_response",
            CUSTOM_PAYLOAD = 22 => "minecraft:custom_payload",
            DEBUG_SUBSCRIPTION_REQUEST = 23 => "minecraft:debug_subscription_request",
            EDIT_BOOK = 24 => "minecraft:edit_book",
            ENTITY_TAG_QUERY = 25 => "minecraft:entity_tag_query",
            INTERACT = 26 => "minecraft:interact",
            JIGSAW_GENERATE = 27 => "minecraft:jigsaw_generate",
            KEEP_ALIVE = 28 => "minecraft:keep_alive",
            LOCK_DIFFICULTY = 29 => "minecraft:lock_difficulty",
            MOVE_PLAYER_POS = 30 => "minecraft:move_player_pos",
            MOVE_PLAYER_POS_ROT = 31 => "minecraft:move_player_pos_rot",
            MOVE_PLAYER_ROT = 32 => "minecraft:move_player_rot",
            MOVE_PLAYER_STATUS_ONLY = 33 => "minecraft:move_player_status_only",
            MOVE_VEHICLE = 34 => "minecraft:move_vehicle",
            PADDLE_BOAT = 35 => "minecraft:paddle_boat",
            PICK_ITEM_FROM_BLOCK = 36 => "minecraft:pick_item_from_block",
            PICK_ITEM_FROM_ENTITY = 37 => "minecraft:pick_item_from_entity",
            PING_REQUEST = 38 => "minecraft:ping_request",
            PLACE_RECIPE = 39 => "minecraft:place_recipe",
            PLAYER_ABILITIES = 40 => "minecraft:player_abilities",
            PLAYER_ACTION = 41 => "minecraft:player_action",
            PLAYER_COMMAND = 42 => "minecraft:player_command",
            PLAYER_INPUT = 43 => "minecraft:player_input",
            PLAYER_LOADED = 44 => "minecraft:player_loaded",
            PONG = 45 => "minecraft:pong",
            PUNCH = 46 => "minecraft:punch",
            RECIPE_BOOK_CHANGE_SETTINGS = 47 => "minecraft:recipe_book_change_settings",
            RECIPE_BOOK_SEEN_RECIPE = 48 => "minecraft:recipe_book_seen_recipe",
            RENAME_ITEM = 49 => "minecraft:rename_item",
            RESOURCE_PACK = 50 => "minecraft:resource_pack",
            SEEN_ADVANCEMENTS = 51 => "minecraft:seen_advancements",
            SELECT_TRADE = 52 => "minecraft:select_trade",
            SET_BEACON = 53 => "minecraft:set_beacon",
            SET_CARRIED_ITEM = 54 => "minecraft:set_carried_item",
            SET_COMMAND_BLOCK = 55 => "minecraft:set_command_block",
            SET_COMMAND_MINECART = 56 => "minecraft:set_command_minecart",
            SET_CREATIVE_MODE_SLOT = 57 => "minecraft:set_creative_mode_slot",
            SET_GAME_RULE = 58 => "minecraft:set_game_rule",
            SET_JIGSAW_BLOCK = 59 => "minecraft:set_jigsaw_block",
            SET_STRUCTURE_BLOCK = 60 => "minecraft:set_structure_block",
            SET_TEST_BLOCK = 61 => "minecraft:set_test_block",
            SIGN_UPDATE = 62 => "minecraft:sign_update",
            SPECTATOR_ACTION = 63 => "minecraft:spectator_action",
            TELEPORT_TO_ENTITY = 64 => "minecraft:teleport_to_entity",
            TEST_INSTANCE_BLOCK_ACTION = 65 => "minecraft:test_instance_block_action",
            USE_ITEM_ON = 66 => "minecraft:use_item_on",
            USE_ITEM = 67 => "minecraft:use_item",
            CUSTOM_CLICK_ACTION = 68 => "minecraft:custom_click_action",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "needs SOLARIS_CLIENT_PACKETS_REPORT=<26.3 packets.json>"]
    fn all_packet_ids_match_26_3_report() {
        let path = std::env::var_os("SOLARIS_CLIENT_PACKETS_REPORT")
            .map(std::path::PathBuf::from)
            .expect("set SOLARIS_CLIENT_PACKETS_REPORT=<26.3 packets.json> to run this acceptance test");
        let report: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read 26.3 packet report {}: {e}", path.display()))
        ).expect("26.3 packet report JSON");
        let groups = [
            ("login", "clientbound", login::clientbound::IDS),
            ("login", "serverbound", login::serverbound::IDS),
            ("configuration", "clientbound", configuration::clientbound::IDS),
            ("configuration", "serverbound", configuration::serverbound::IDS),
            ("play", "clientbound", play::clientbound::IDS),
            ("play", "serverbound", play::serverbound::IDS),
        ];
        let mut failures = Vec::new();
        let mut checked = 0;
        for (phase, direction, ids) in groups {
            for &(name, id) in ids {
                let actual = report[phase][direction][name]["protocol_id"].as_i64();
                if actual != Some(i64::from(id)) {
                    failures.push(format!("{phase}/{direction}/{name}: client={id}, report={actual:?}"));
                }
                checked += 1;
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        eprintln!("checked {checked} named packet IDs against {}", path.display());
    }
}
