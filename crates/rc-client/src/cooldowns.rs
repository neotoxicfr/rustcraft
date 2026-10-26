//! Item cooldowns on the client side (packet `cooldown`): per group, start and end in the
//! player's ticks. The group of an item is that of its `use_cooldown` component, otherwise its
//! item identifier; it is up to the caller to supply it.

use hashbrown::HashMap;

#[derive(Debug, Default, Clone)]
pub struct Cooldowns {
    groups: HashMap<String, (i32, i32)>,
    ticks: i32,
}

impl Cooldowns {
    /// Packet `cooldown`: zero duration = cooldown removed, otherwise (re)started now.
    pub fn apply(&mut self, group: &str, ticks: i32) {
        if ticks == 0 {
            self.groups.remove(group);
        } else {
            self.groups.insert(group.to_owned(), (self.ticks, self.ticks.wrapping_add(ticks)));
        }
    }

    /// One player tick: expired cooldowns disappear.
    pub fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        let now = self.ticks;
        self.groups.retain(|_, &mut (_, end)| end > now);
    }

    /// Remaining fraction (1 → 0) of the group's cooldown, to within `partial` tick.
    pub fn percent(&self, group: &str, partial: f32) -> f32 {
        match self.groups.get(group) {
            Some(&(start, end)) => {
                let duration = (end - start) as f32;
                let remaining = end as f32 - (self.ticks as f32 + partial);
                (remaining / duration).clamp(0.0, 1.0)
            }
            None => 0.0,
        }
    }

    pub fn is_on_cooldown(&self, group: &str) -> bool {
        self.percent(group, 0.0) > 0.0
    }

    /// World or player change: everything is forgotten (new local player).
    pub fn clear(&mut self) {
        self.groups.clear();
        self.ticks = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_game_timeline() {
        let mut c = Cooldowns::default();
        c.apply("minecraft:ender_pearl", 20);
        assert_eq!(c.percent("minecraft:ender_pearl", 0.0), 1.0);
        for _ in 0..10 {
            c.tick();
        }
        assert!((c.percent("minecraft:ender_pearl", 0.5) - 0.475).abs() < 1e-6);
        for _ in 0..10 {
            c.tick();
        }
        assert!(!c.is_on_cooldown("minecraft:ender_pearl"));
        c.apply("minecraft:shield", 100);
        c.apply("minecraft:shield", 0);
        assert_eq!(c.percent("minecraft:shield", 0.0), 0.0);
    }
}
