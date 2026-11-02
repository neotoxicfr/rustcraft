//! Success messages relayed to the other operators (`chat.type.admin`, grey italic): the game
//! decides it per call; every success is relayed except the queries and listings below (sorted
//! translation keys of the game's calls that keep their message for the source). A success
//! that is not a translation (debug output, help lines) is never relayed.

use rc_text::{Component, Content};

/// Translation keys of the successes kept for the source alone (sorted, binary search).
pub(crate) const QUIET: [&str; 100] = [
    "command.compute.result.named.exact",
    "command.compute.result.named.invalid",
    "command.compute.result.named.rounded",
    "command.compute.result.unnamed.exact",
    "command.compute.result.unnamed.invalid",
    "command.compute.result.unnamed.rounded",
    "commands.attribute.base_value.get.success",
    "commands.attribute.base_value.reset.success",
    "commands.attribute.base_value.set.success",
    "commands.attribute.modifier.add.success",
    "commands.attribute.modifier.remove.success",
    "commands.attribute.modifier.value.get.success",
    "commands.attribute.value.get.success",
    "commands.banlist.entry",
    "commands.banlist.list",
    "commands.banlist.none",
    "commands.bossbar.list.bars.none",
    "commands.bossbar.list.bars.some",
    "commands.data.block.get",
    "commands.data.block.query",
    "commands.data.entity.get",
    "commands.data.entity.query",
    "commands.data.storage.get",
    "commands.data.storage.query",
    "commands.datapack.list.available.none",
    "commands.datapack.list.available.success",
    "commands.datapack.list.enabled.none",
    "commands.datapack.list.enabled.success",
    "commands.difficulty.query",
    "commands.drop.success.multiple",
    "commands.drop.success.multiple_with_table",
    "commands.drop.success.single",
    "commands.drop.success.single_with_table",
    "commands.execute.conditional.pass",
    "commands.execute.conditional.pass_count",
    "commands.experience.query.levels",
    "commands.experience.query.points",
    "commands.fetchprofile.entity.success",
    "commands.fetchprofile.id.success",
    "commands.fetchprofile.name.success",
    "commands.forceload.list.multiple",
    "commands.forceload.list.single",
    "commands.forceload.query.success",
    "commands.gamerule.query",
    "commands.jfr.started",
    "commands.jfr.stopped",
    "commands.list.players",
    "commands.locate.biome.success",
    "commands.locate.poi.success",
    "commands.locate.structure.success",
    "commands.perf.reportSaved",
    "commands.perf.started",
    "commands.perf.stopped",
    "commands.posteffect.list.empty",
    "commands.posteffect.list.success",
    "commands.random.reset.all.success",
    "commands.random.reset.success",
    "commands.random.sample.success",
    "commands.save.saving",
    "commands.scoreboard.objectives.list.empty",
    "commands.scoreboard.objectives.list.success",
    "commands.scoreboard.players.get.success",
    "commands.scoreboard.players.list.empty",
    "commands.scoreboard.players.list.entity.empty",
    "commands.scoreboard.players.list.entity.entry",
    "commands.scoreboard.players.list.entity.success",
    "commands.scoreboard.players.list.success",
    "commands.seed.success",
    "commands.spectate.success.started",
    "commands.spectate.success.stopped",
    "commands.tag.list.multiple.empty",
    "commands.tag.list.multiple.success",
    "commands.tag.list.single.empty",
    "commands.tag.list.single.success",
    "commands.team.list.members.empty",
    "commands.team.list.members.success",
    "commands.team.list.teams.empty",
    "commands.team.list.teams.success",
    "commands.team.option.prefix.success",
    "commands.team.option.suffix.success",
    "commands.test.locate.found",
    "commands.test.no_tests",
    "commands.test.relative_position",
    "commands.test.run.running",
    "commands.tick.query.percentiles",
    "commands.tick.query.rate.running",
    "commands.tick.query.rate.sprinting",
    "commands.tick.status.lagging",
    "commands.time.query.absolute",
    "commands.time.query.gametime",
    "commands.time.query.timeline",
    "commands.time.query.timeline.repetitions",
    "commands.waypoint.list.empty",
    "commands.waypoint.list.success",
    "commands.waypoint.modify.color",
    "commands.waypoint.modify.color.reset",
    "commands.waypoint.modify.style",
    "commands.whitelist.list",
    "commands.whitelist.none",
    "commands.worldborder.get",
];

/// Is this success kept for its source alone?
pub(crate) fn quiet(msg: &Component) -> bool {
    match &msg.content {
        Content::Translate { key, .. } => QUIET.binary_search(&key.as_str()).is_ok(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_classifies_messages() {
        assert!(QUIET.windows(2).all(|w| w[0] < w[1]), "QUIET must stay sorted for the binary search");
        let t = |k: &str| Component::translate(k, Vec::new());
        // Queries and listings stay with the source.
        for k in [
            "commands.time.query.gametime",
            "commands.seed.success",
            "commands.data.entity.query",
            "commands.list.players",
            "commands.fetchprofile.name.success",
            "commands.fetchprofile.id.success",
            "commands.fetchprofile.entity.success",
        ] {
            assert!(quiet(&t(k)), "{k} must not be relayed");
        }
        // Changes are relayed (the same key can be both: tick status, decided by its caller).
        for k in ["commands.time.set", "commands.give.success.single", "commands.tick.status.frozen", "commands.test.summary"] {
            assert!(!quiet(&t(k)), "{k} must be relayed");
        }
        assert!(quiet(&Component::text("/help line")));
    }
}
