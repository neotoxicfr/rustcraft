//! Tooltip lines against the component id lookups: the same lines whichever way the ids are
//! found, and what each way costs per tooltip.

use rc_proto_v777::item::ItemStack;

use crate::stack::ComponentLookup;
use crate::tooltips::test_fixture::TooltipWorld;

const LOOKUPS: [(&str, ComponentLookup); 4] = [
    ("walk over the names (former)", ComponentLookup::FORMER),
    ("full name, rc-data walk", ComponentLookup { by_full_name: true, hashed: false, resolved_table: false }),
    ("full name, hashed", ComponentLookup { by_full_name: true, hashed: true, resolved_table: false }),
    ("hashed + table of resolved ids", ComponentLookup { by_full_name: true, hashed: true, resolved_table: true }),
];

/// Lines of a stack's tooltip as text and styles, normal then advanced, then its search lines.
fn tooltip_of(world: &TooltipWorld, stack: &ItemStack, creative: bool) -> (Vec<Vec<rc_text::StyledChar>>, Vec<String>) {
    let it = world.items();
    crate::tooltips_more::set_creative(creative);
    let mut lines = crate::screens::tooltip_lines(stack, &it, &world.enchantments, &world.english, false);
    lines.extend(crate::screens::tooltip_lines(stack, &it, &world.enchantments, &world.english, true));
    let search = crate::screens::tooltip_search_lines(stack, &it, &world.enchantments, &world.english);
    crate::tooltips_more::set_creative(false);
    (lines, search)
}

fn text_of(lines: &[Vec<rc_text::StyledChar>]) -> Vec<String> {
    lines.iter().map(|line| line.iter().map(|c| c.c).collect()).collect()
}

/// One plain stack of every item of the registry.
fn one_of_every_item(world: &TooltipWorld) -> Vec<ItemStack> {
    (0..world.item_names.len() as i32)
        .map(|item| ItemStack { item, count: 1, added: Vec::new(), removed: Vec::new() })
        .collect()
}

#[test]
fn tooltip_lines_are_the_same_on_every_component_lookup() {
    let _one_at_a_time = crate::tooltips::test_fixture::world_inputs_lock();
    let former = TooltipWorld::with_lookup(ComponentLookup::FORMER);
    let stacks: Vec<ItemStack> =
        former.stacks().into_iter().map(|(_, stack)| stack).chain(one_of_every_item(&former)).collect();
    assert!(stacks.len() > 1_000, "{} stacks", stacks.len());
    for (label, lookup) in &LOOKUPS[1..] {
        let world = TooltipWorld::with_lookup(*lookup);
        for stack in &stacks {
            for creative in [false, true] {
                let item = &former.item_names[stack.item as usize];
                let (found, expected) = (tooltip_of(&world, stack, creative), tooltip_of(&former, stack, creative));
                // The text first: a short failure message.
                assert_eq!(text_of(&found.0), text_of(&expected.0), "{label}: {item}");
                assert!(found == expected, "{label}: {item}: same text, other styles or search lines");
            }
        }
    }
}

/// Micro-bench (ignored): `cargo test --release -p rc-app bench_tooltip_build -- --ignored
/// --nocapture`. Tooltip lines of the fixture stacks, then the search lines of one stack of every
/// item (what a creative search index builds), on each lookup path.
#[test]
#[ignore = "micro-bench"]
fn bench_tooltip_build() {
    const ROUNDS: usize = 200;
    for (label, lookup) in LOOKUPS {
        let world = TooltipWorld::with_lookup(lookup);
        let it = world.items();
        let stacks: Vec<ItemStack> = world.stacks().into_iter().map(|(_, stack)| stack).collect();
        let per_tooltip = (0..5)
            .map(|_| {
                let start = std::time::Instant::now();
                for _ in 0..ROUNDS {
                    for stack in &stacks {
                        std::hint::black_box(crate::screens::tooltip_lines(
                            stack,
                            &it,
                            &world.enchantments,
                            &world.english,
                            false,
                        ));
                    }
                }
                start.elapsed().as_nanos() as f64 / (ROUNDS * stacks.len()) as f64
            })
            .fold(f64::MAX, f64::min);
        let every_item = one_of_every_item(&world);
        let index_build = (0..5)
            .map(|_| {
                let start = std::time::Instant::now();
                for stack in &every_item {
                    std::hint::black_box(crate::screens::tooltip_search_lines(
                        stack,
                        &it,
                        &world.enchantments,
                        &world.english,
                    ));
                }
                start.elapsed().as_secs_f64() * 1e3
            })
            .fold(f64::MAX, f64::min);
        println!(
            "{label:<31} {per_tooltip:>8.0} ns per tooltip, {index_build:>6.2} ms for the search lines of {} items",
            every_item.len()
        );
    }
}
