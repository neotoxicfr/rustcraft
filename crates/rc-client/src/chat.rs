//! Received chat (`docs/specs/chat.md` § 1): chat types, decoration, player messages on an
//! offline server (unsigned, accepted).

use rc_proto::nbt::Nbt;
use rc_proto_v777::play::{ChatTypeRef, Decoration, PlayerChat};
use rc_text::{Arg, Component};

use crate::ChatTag;

/// Vanilla types, when the server does not send the registry content (known pack).
fn vanilla(name: &str) -> Decoration {
    let (key, params, gray_italic): (&str, &[u8], bool) = match name {
        "minecraft:say_command" => ("chat.type.announcement", &[0, 2], false),
        "minecraft:msg_command_incoming" => ("commands.message.display.incoming", &[0, 2], true),
        "minecraft:msg_command_outgoing" => ("commands.message.display.outgoing", &[1, 2], true),
        "minecraft:team_msg_command_incoming" => ("chat.type.team.text", &[1, 0, 2], false),
        "minecraft:team_msg_command_outgoing" => ("chat.type.team.sent", &[1, 0, 2], false),
        "minecraft:emote_command" => ("chat.type.emote", &[0, 2], false),
        _ => ("chat.type.text", &[0, 2], false),
    };
    let style = gray_italic
        .then(|| Nbt::Compound(vec![("color".into(), Nbt::String("gray".into())), ("italic".into(), Nbt::Byte(1))]));
    Decoration { key: key.into(), params: params.to_vec(), style }
}

/// Decoration of an entry of the `chat_type` registry (NBT content, or vanilla by name).
pub fn chat_type(name: &str, nbt: Option<&Nbt>) -> Decoration {
    let Some(chat) = nbt.and_then(|n| n.get("chat")) else { return vanilla(name) };
    let key = chat.get("translation_key").and_then(Nbt::as_str).unwrap_or("chat.type.text").to_owned();
    let params = match chat.get("parameters") {
        Some(Nbt::List(l)) => l
            .iter()
            .filter_map(Nbt::as_str)
            .map(|p| match p {
                "target" => 1,
                "content" => 2,
                _ => 0,
            })
            .collect(),
        _ => vec![0, 2],
    };
    Decoration { key, params, style: chat.get("style").cloned() }
}

/// Decorated message: translation of the key with the parameters, style of the decoration.
pub fn decorate(
    kind: &ChatTypeRef,
    types: &[Decoration],
    content: Component,
    sender: Option<&Nbt>,
    target: Option<&Nbt>,
) -> Component {
    let fallback;
    let d = match kind {
        ChatTypeRef::Registry(i) => match types.get(*i as usize) {
            Some(d) => d,
            None => {
                fallback = vanilla("");
                &fallback
            }
        },
        ChatTypeRef::Inline(d) => d,
    };
    let comp = |n: Option<&Nbt>| n.and_then(rc_text::from_nbt).unwrap_or_else(|| Component::text(""));
    let with = d
        .params
        .iter()
        .map(|p| match p {
            1 => Arg::Component(comp(target)),
            2 => Arg::Component(content.clone()),
            _ => Arg::Component(comp(sender)),
        })
        .collect();
    let mut c = Component::translate(d.key.clone(), with);
    if let Some(s) = d.style.as_ref().and_then(rc_text::from_nbt) {
        c.style = s.style;
    }
    c
}

/// Chat line of a player message ready to display: component, tag, and original signed text
/// for the "modified" tag.
pub struct PlayerLine {
    pub component: Component,
    pub tag: ChatTag,
    pub original: Option<String>,
}

/// Player message after validation (`chat.md` § 1.8), like the game: decoration, trust
/// level (secure, modified, not secure), hiding (secure chat only, hidden player,
/// everything filtered, player chat off) and filtered text. `visible`: the chat visibility
/// lets player messages through. Returns the line (or nothing) and "displayed".
pub fn player_message(
    m: &PlayerChat,
    types: &[Decoration],
    verdict: crate::chat_verify::Verdict,
    visible: bool,
    now_ms: i64,
) -> (Option<PlayerLine>, bool) {
    use crate::chat_verify::Verdict;
    let decorate_with =
        |content: Component| decorate(&m.chat_type, types, content, m.sender_name.as_ref(), m.target_name.as_ref());
    let signed = match verdict {
        Verdict::Rejected => {
            if should_hide(m.sender) || friend_only_restricted(m.sender) || !visible {
                return (None, false);
            }
            let mut error = Component::translate("chat.validation_error", Vec::new());
            error.style.color = Some(0xFF5555);
            error.style.italic = Some(true);
            let line = PlayerLine { component: decorate_with(error), tag: ChatTag::Error, original: None };
            return (Some(line), true);
        }
        Verdict::Accepted { signed } => signed,
    };
    let only_secure = only_secure();
    let unsigned = m.unsigned.as_ref().map(|n| rc_text::from_nbt(n).unwrap_or_else(|| Component::text("")));
    // Secure chat only: the content decorated by the server is ignored.
    let displayed = if only_secure { None } else { unsigned.clone() };
    let decorated = decorate_with(displayed.unwrap_or_else(|| Component::text(m.content.clone())));
    let tag = if !signed || now_ms > m.timestamp_ms.saturating_add(crate::chat_verify::CLIENT_EXPIRY_MS) {
        ChatTag::NotSecure
    } else if !decorated_text(&decorated).contains(&m.content)
        || unsigned.as_ref().is_some_and(|u| modified_font(u, None))
    {
        ChatTag::Modified
    } else {
        ChatTag::Secure
    };
    if (only_secure && tag == ChatTag::NotSecure)
        || should_hide(m.sender)
        || m.filter.0 == 1
        || friend_only_restricted(m.sender)
        || !visible
    {
        return (None, false);
    }
    let component = if m.filter.0 == 2 { decorate_with(filtered(&m.content, &m.filter.1)) } else { decorated };
    let original = (tag == ChatTag::Modified).then(|| m.content.clone());
    (Some(PlayerLine { component, tag, original }), true)
}

/// Raw text of the decorated message for the "modified" check: decoration parameters put
/// end to end (only they carry content; the translation only adds
/// separators).
fn decorated_text(decorated: &Component) -> String {
    let lang = rc_text::Lang::default();
    let mut out = String::new();
    if let rc_text::Content::Translate { with, .. } = &decorated.content {
        for a in with {
            match a {
                Arg::Component(c) => out.push_str(&rc_text::plain_string(c, &lang)),
                Arg::Text(s) => out.push_str(s),
            }
            out.push(' ');
        }
    }
    out
}

/// A font other than the default somewhere in the content (inherited style).
fn modified_font(c: &Component, parent: Option<&str>) -> bool {
    let font = c.style.font.as_deref().or(parent);
    if font.is_some_and(|f| f != "minecraft:default" && f != "default") {
        return true;
    }
    let in_args = match &c.content {
        rc_text::Content::Translate { with, .. } => with.iter().any(|a| match a {
            Arg::Component(a) => modified_font(a, font),
            Arg::Text(_) => false,
        }),
        _ => false,
    };
    in_args || c.extra.iter().any(|e| modified_font(e, font))
}

/// Partly filtered signed text: each hidden passage becomes dark gray `#`s with
/// the `chat.filtered` tooltip; mask bits indexed in UTF-16 units, like the game.
pub fn filtered(text: &str, mask: &[u8]) -> Component {
    let bit = |i: usize| mask.get(i / 8).is_some_and(|b| b >> (i % 8) & 1 == 1);
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut root = Component::text("");
    let mut start = 0usize;
    while start < units.len() {
        let filtered = bit(start);
        let mut end = start;
        while end < units.len() && bit(end) == filtered {
            end += 1;
        }
        if filtered {
            let mut c = Component::text("#".repeat(end - start));
            c.style.color = Some(0x555555);
            c.style.hover =
                Some(std::sync::Arc::new(rc_text::HoverEvent::Text(Component::translate("chat.filtered", Vec::new()))));
            root.extra.push(c);
        } else {
            root.extra.push(Component::text(String::from_utf16_lossy(&units[start..end])));
        }
        start = end;
    }
    root
}

/// "Secure chat only" setting, applied by the game to each received message.
static ONLY_SECURE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_only_secure(on: bool) {
    ONLY_SECURE.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn only_secure() -> bool {
    ONLY_SECURE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Chat visibility (`chatVisibility`): 0 everything, 1 system only, 2 hidden.
static VISIBILITY: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

pub fn set_visibility(v: i32) {
    VISIBILITY.store(v, std::sync::atomic::Ordering::Relaxed);
}

pub fn visibility() -> i32 {
    VISIBILITY.load(std::sync::atomic::Ordering::Relaxed)
}

pub type Signature = Box<[u8; 256]>;

/// Hidden players (social interactions screen): their messages are not displayed.
/// Valid for the duration of the launch, like the game.
static HIDDEN: std::sync::Mutex<Vec<u128>> = std::sync::Mutex::new(Vec::new());

pub fn set_hidden(uuid: u128, hidden: bool) {
    let mut h = HIDDEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    h.retain(|u| *u != uuid);
    if hidden {
        h.push(uuid);
    }
}

pub fn is_hidden(uuid: u128) -> bool {
    HIDDEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains(&uuid)
}

/// Messages of this player to hide: hidden in the social interactions, or blocked by the
/// account (block list, online only), like the game.
pub fn should_hide(uuid: u128) -> bool {
    is_hidden(uuid) || crate::session::is_blocked(uuid)
}

/// Hidden players, in the order they were hidden.
pub fn hidden_players() -> Vec<u128> {
    HIDDEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// Usernames seen in the player list → identifier (never removed, like the game): used to
/// guess the sender of a system message.
static DISCOVERED: std::sync::Mutex<Option<hashbrown::HashMap<String, u128>>> = std::sync::Mutex::new(None);

pub fn discover(name: &str, uuid: u128) {
    let mut d = DISCOVERED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    d.get_or_insert_with(Default::default).insert(name.to_owned(), uuid);
}

fn discovered(name: &str) -> u128 {
    let d = DISCOVERED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    d.as_ref().and_then(|m| m.get(name).copied()).unwrap_or(0)
}

/// Identifier of the local player (profile of the current connection).
static LOCAL: std::sync::Mutex<u128> = std::sync::Mutex::new(0);

pub fn set_local(uuid: u128) {
    *LOCAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = uuid;
}

fn is_local(uuid: u128) -> bool {
    *LOCAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner) == uuid
}

/// `hideMatchedNames` setting: hide system messages whose guessed sender is hidden.
static HIDE_MATCHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_hide_matched_names(on: bool) {
    HIDE_MATCHED.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Player whose messages are hidden (hidden or blocked), except the local player and the nil identifier.
pub fn is_blocked(uuid: u128) -> bool {
    uuid != 0 && !is_local(uuid) && should_hide(uuid)
}

/// Account limited to chat between friends: only friends (active friends list) are read. Rustcraft
/// keeps no friends list: no player is a friend, so all the others are hidden.
pub fn friend_only_restricted(uuid: u128) -> bool {
    crate::session::user_properties().flag(crate::session::flag::CHAT_FRIENDS_ONLY) && !is_local(uuid)
}

/// Is a system message displayed? Sender guessed from the raw text (known `<username>`):
/// hidden if blocked (with `hideMatchedNames`) or if the account is limited to friends.
pub fn system_message_shown(c: &Component) -> bool {
    let text = rc_text::plain_string(c, &rc_text::Lang::default());
    let guessed = text.split_once('<').and_then(|(_, r)| r.split_once('>')).map_or(0, |(n, _)| discovered(n));
    if HIDE_MATCHED.load(std::sync::atomic::Ordering::Relaxed) && is_blocked(guessed) {
        return false;
    }
    guessed == 0 || !friend_only_restricted(guessed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_verify::Verdict;

    fn msg(content: &str) -> PlayerChat {
        PlayerChat {
            global_index: 0,
            sender: 5,
            session_index: 0,
            signature: Some(Box::new([1; 256])),
            content: content.into(),
            timestamp_ms: 1_000,
            salt: 0,
            last_seen: Vec::new(),
            unsigned: None,
            filter: (0, Vec::new()),
            chat_type: ChatTypeRef::Registry(0),
            sender_name: Some(Nbt::String("Alex".into())),
            target_name: None,
        }
    }

    fn types() -> Vec<Decoration> {
        vec![vanilla("minecraft:chat")]
    }

    fn line(m: &PlayerChat, v: Verdict, now: i64) -> Option<PlayerLine> {
        player_message(m, &types(), v, true, now).0
    }

    #[test]
    fn trust_levels_like_the_game() {
        let signed = Verdict::Accepted { signed: true };
        // Signed, verified, recent: no bar.
        assert_eq!(line(&msg("hello"), signed, 2_000).unwrap().tag, ChatTag::Secure);
        // Received more than 7 min after its timestamp: not secure.
        assert_eq!(line(&msg("hello"), signed, 1_000 + 7 * 60_000 + 1).unwrap().tag, ChatTag::NotSecure);
        // Accepted without signature (no session): not secure.
        assert_eq!(line(&msg("hello"), Verdict::Accepted { signed: false }, 2_000).unwrap().tag, ChatTag::NotSecure);
        // Content decorated by the server without the signed text: modified, original text kept.
        let mut m = msg("hello");
        m.unsigned = Some(Nbt::String("hi".into()));
        let l = line(&m, signed, 2_000).unwrap();
        assert_eq!((l.tag, l.original.as_deref()), (ChatTag::Modified, Some("hello")));
        // Decorated content that contains the signed text, default font: secure.
        m.unsigned = Some(Nbt::String("hello!".into()));
        assert_eq!(line(&m, signed, 2_000).unwrap().tag, ChatTag::Secure);
        // Font changed: modified.
        m.unsigned = Some(Nbt::Compound(vec![
            ("text".into(), Nbt::String("hello".into())),
            ("font".into(), Nbt::String("minecraft:alt".into())),
        ]));
        assert_eq!(line(&m, signed, 2_000).unwrap().tag, ChatTag::Modified);
    }

    #[test]
    fn rejected_and_hidden_messages() {
        let l = line(&msg("x"), Verdict::Rejected, 0).unwrap();
        assert_eq!(l.tag, ChatTag::Error);
        let rc_text::Content::Translate { with, .. } = &l.component.content else { panic!() };
        let Arg::Component(c) = &with[1] else { panic!() };
        assert_eq!(
            c.content,
            rc_text::Content::Translate { key: "chat.validation_error".into(), fallback: None, with: vec![] }
        );
        assert_eq!((c.style.color, c.style.italic), (Some(0xFF5555), Some(true)));
        // Everything filtered, or player chat off: nothing, and "not displayed".
        let mut m = msg("x");
        m.filter = (1, Vec::new());
        assert!(player_message(&m, &types(), Verdict::Accepted { signed: true }, true, 0).0.is_none());
        assert!(!player_message(&msg("x"), &types(), Verdict::Accepted { signed: true }, false, 0).1);
    }

    #[test]
    fn partial_filter_uses_utf16_units_and_style() {
        // "é😀ab": é = 1 unit, 😀 = 2 units; mask of units 1 and 2 (the emoji).
        let c = filtered("é😀ab", &[0b0000_0110]);
        let texts: Vec<_> = c
            .extra
            .iter()
            .map(|e| match &e.content {
                rc_text::Content::Text(t) => (t.clone(), e.style.color),
                _ => panic!(),
            })
            .collect();
        assert_eq!(texts, vec![("é".into(), None), ("##".into(), Some(0x555555)), ("ab".into(), None)]);
        assert!(matches!(c.extra[1].style.hover.as_deref(), Some(rc_text::HoverEvent::Text(_))));
    }

    #[test]
    fn system_message_from_hidden_player_is_dropped() {
        discover("MutedSys", 0x7701);
        let line = Component::text("<MutedSys> hello");
        assert!(system_message_shown(&line));
        set_hidden(0x7701, true);
        assert!(!system_message_shown(&line));
        // Setting off: the message goes through.
        set_hide_matched_names(false);
        assert!(system_message_shown(&line));
        set_hide_matched_names(true);
        // Unknown or chevron-less username: never hidden.
        assert!(system_message_shown(&Component::text("<Unknown> hello")));
        assert!(system_message_shown(&Component::text("MutedSys hello")));
        // Local player: never hidden.
        set_local(0x7701);
        assert!(system_message_shown(&line));
        set_local(0);
        set_hidden(0x7701, false);
    }
}
