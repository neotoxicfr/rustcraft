//! Scoreboard and teams (`docs/specs/scoreboard-teams.md`): objectives, display
//! slots, scores, teams.

use rc_proto::nbt::Nbt;
use rc_proto::varint::read_varint;
use rc_proto::{Decode, Reader, Result, read_count, read_str};

/// Number format (§ 2).
#[derive(Debug, Clone, PartialEq)]
pub enum NumberFormat {
    Blank,
    /// Style alone (NBT compound of the style fields).
    Styled(Option<Nbt>),
    Fixed(Option<Nbt>),
}

fn read_format(r: &mut Reader<'_>) -> Result<NumberFormat> {
    Ok(match read_varint(r)? {
        1 => NumberFormat::Styled(rc_proto::nbt::read(r)?),
        2 => NumberFormat::Fixed(rc_proto::nbt::read(r)?),
        _ => NumberFormat::Blank,
    })
}

fn read_opt_format(r: &mut Reader<'_>) -> Result<Option<NumberFormat>> {
    if bool::decode(r)? { read_format(r).map(Some) } else { Ok(None) }
}

fn read_opt_nbt(r: &mut Reader<'_>) -> Result<Option<Nbt>> {
    if bool::decode(r)? { rc_proto::nbt::read(r) } else { Ok(None) }
}

/// Team parameters (modes 0 and 2).
#[derive(Debug, Clone, PartialEq)]
pub struct TeamParams {
    pub display: Option<Nbt>,
    pub prefix: Option<Nbt>,
    pub suffix: Option<Nbt>,
    /// 0 always, 1 never, 2 hideForOtherTeams, 3 hideForOwnTeam.
    pub nametag: i32,
    /// 0 always, 1 never, 2 pushOtherTeams, 3 pushOwnTeam.
    pub collision: i32,
    /// Colour 0..15; absent = no colour.
    pub color: Option<i32>,
    /// 0x01 friendly fire, 0x02 sees invisible allies.
    pub options: i8,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScoreEvent {
    /// Mode 0 create, 1 remove, 2 update; display name, hearts, format.
    Objective {
        name: String,
        mode: i8,
        display: Option<Nbt>,
        hearts: bool,
        format: Option<NumberFormat>,
    },
    Display {
        slot: i32,
        objective: String,
    },
    Score {
        holder: String,
        objective: String,
        value: i32,
        display: Option<Nbt>,
        format: Option<NumberFormat>,
    },
    Reset {
        holder: String,
        objective: Option<String>,
    },
    Team {
        name: String,
        mode: i8,
        params: Option<TeamParams>,
        members: Vec<String>,
    },
}

pub fn read_set_objective(r: &mut Reader<'_>) -> Result<ScoreEvent> {
    let name = read_str(r, 32767)?.to_owned();
    let mode = i8::decode(r)?;
    let (mut display, mut hearts, mut format) = (None, false, None);
    if mode == 0 || mode == 2 {
        display = rc_proto::nbt::read(r)?;
        hearts = read_varint(r)? == 1;
        format = read_opt_format(r)?;
    }
    Ok(ScoreEvent::Objective { name, mode, display, hearts, format })
}

pub fn read_display_objective(r: &mut Reader<'_>) -> Result<ScoreEvent> {
    let slot = read_varint(r)?;
    let slot = if (0..19).contains(&slot) { slot } else { 0 };
    Ok(ScoreEvent::Display { slot, objective: read_str(r, 32767)?.to_owned() })
}

pub fn read_set_score(r: &mut Reader<'_>) -> Result<ScoreEvent> {
    Ok(ScoreEvent::Score {
        holder: read_str(r, 32767)?.to_owned(),
        objective: read_str(r, 32767)?.to_owned(),
        value: read_varint(r)?,
        display: read_opt_nbt(r)?,
        format: read_opt_format(r)?,
    })
}

pub fn read_reset_score(r: &mut Reader<'_>) -> Result<ScoreEvent> {
    let holder = read_str(r, 32767)?.to_owned();
    let objective = if bool::decode(r)? { Some(read_str(r, 32767)?.to_owned()) } else { None };
    Ok(ScoreEvent::Reset { holder, objective })
}

pub fn read_player_team(r: &mut Reader<'_>) -> Result<ScoreEvent> {
    let name = read_str(r, 32767)?.to_owned();
    let mode = i8::decode(r)?;
    let params = if mode == 0 || mode == 2 {
        let display = rc_proto::nbt::read(r)?;
        let prefix = rc_proto::nbt::read(r)?;
        let suffix = rc_proto::nbt::read(r)?;
        let nametag = read_varint(r)?;
        let collision = read_varint(r)?;
        let color = if bool::decode(r)? {
            let c = read_varint(r)?;
            Some(if (0..16).contains(&c) { c } else { 0 })
        } else {
            None
        };
        let options = i8::decode(r)?;
        Some(TeamParams { display, prefix, suffix, nametag, collision, color, options })
    } else {
        None
    };
    let members = if matches!(mode, 0 | 3 | 4) {
        let n = read_count(r, 1)?;
        (0..n).map(|_| read_str(r, 32767).map(str::to_owned)).collect::<Result<_>>()?
    } else {
        Vec::new()
    };
    Ok(ScoreEvent::Team { name, mode, params, members })
}
