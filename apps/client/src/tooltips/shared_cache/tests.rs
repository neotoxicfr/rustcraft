//! Proofs of the shared tooltip store: the kept tooltip is the one a rebuild gives, for stacks
//! covering the tooltip sections, after every change of what a tooltip reads (flags, language,
//! fonts, tooltip context, tags, registries, map table), and over frames of alternating hover.

use super::*;
use crate::tooltips::test_fixture::{BREAKABLE_TAG, FixedWidths, MAP_ID, TooltipWorld, world_inputs_lock};

/// One state of the interface text: language and glyph widths.
struct Text<'a> {
    lang: &'a rc_text::Lang,
    widths: FixedWidths,
    font_generation: usize,
}

impl Text<'_> {
    fn source(&self) -> TextSource<'_> {
        TextSource { lang: self.lang, metrics: &self.widths, font_generation: self.font_generation }
    }
}

const FLAGS: [(bool, bool); 4] = [(false, false), (true, false), (false, true), (true, true)];

/// Tooltip of `stack` from the store, for the given (advanced, creative) flags.
fn kept(
    world: &TooltipWorld,
    stack: &ItemStack,
    text: &Text<'_>,
    (advanced, creative): (bool, bool),
) -> Rc<ItemTooltip> {
    let it = world.items();
    let request = Request { stack, it: &it, enchantments: &world.enchantments, advanced, creative };
    kept_or_built(&request, &text.source(), true)
}

fn rebuilt(
    world: &TooltipWorld,
    stack: &ItemStack,
    text: &Text<'_>,
    (advanced, creative): (bool, bool),
) -> ItemTooltip {
    let it = world.items();
    build(&Request { stack, it: &it, enchantments: &world.enchantments, advanced, creative }, &text.source())
}

/// Every stack under every flag pair: the kept tooltip equals a rebuild made now.
fn assert_kept_equals_rebuilt(world: &TooltipWorld, stacks: &[(&str, ItemStack)], text: &Text<'_>, after: &str) {
    for (label, stack) in stacks {
        for flags in FLAGS {
            let (kept, rebuilt) = (kept(world, stack, text, flags), rebuilt(world, stack, text, flags));
            assert_same_tooltip(&kept, &rebuilt, &format!("{label}, (advanced, creative) {flags:?}, {after}"));
        }
    }
}

fn text_of(tooltip: &ItemTooltip) -> Vec<String> {
    tooltip.lines.iter().map(|line| line.iter().map(|c| c.c).collect()).collect()
}

/// Equal tooltips: the text first (a short failure message), then the styles, the width and
/// the tooltip style.
fn assert_same_tooltip(kept: &ItemTooltip, rebuilt: &ItemTooltip, context: &str) {
    assert_eq!(text_of(kept), text_of(rebuilt), "{context}");
    assert_eq!((kept.width, &kept.style), (rebuilt.width, &rebuilt.style), "{context}");
    assert!(kept == rebuilt, "{context}: same text, other character styles");
}

fn stack_named<'a>(stacks: &'a [(&str, ItemStack)], label: &str) -> &'a ItemStack {
    &stacks.iter().find(|(l, _)| *l == label).unwrap_or_else(|| panic!("no stack labelled {label}")).1
}

/// The kept tooltip of `label` after `change`: must differ from the one before (the change is
/// one the tooltip shows) and equal a rebuild, like every other stack.
fn assert_change_is_followed(
    world: &TooltipWorld,
    stacks: &[(&str, ItemStack)],
    text: &Text<'_>,
    (label, flags): (&str, (bool, bool)),
    what: &str,
    change: impl FnOnce(),
) {
    let stack = stack_named(stacks, label);
    let before = kept(world, stack, text, flags);
    change();
    let after = kept(world, stack, text, flags);
    assert_ne!(text_of(&before), text_of(&after), "{what}: the tooltip of the {label} must show it");
    assert_kept_equals_rebuilt(world, stacks, text, what);
}

fn enchantment_tags(order: &[i32], curses: &[i32]) -> rc_proto_v777::play::Tags {
    let tags =
        vec![("minecraft:tooltip_order".to_owned(), order.to_vec()), ("minecraft:curse".to_owned(), curses.to_vec())];
    vec![("minecraft:enchantment".to_owned(), tags)]
}

fn map_packet(id: i32) -> rc_proto_v777::map::MapItemData {
    rc_proto_v777::map::MapItemData { id, scale: 1, locked: false, decorations: None, patch: None }
}

#[test]
fn kept_tooltip_follows_every_input_change() {
    let _one_at_a_time = world_inputs_lock();
    let world = TooltipWorld::new();
    let stacks = world.stacks();
    let english = Text { lang: &world.english, widths: FixedWidths { space: 4.0 }, font_generation: 1 };
    crate::tooltips::set_context(false, false, 20.0);
    crate::tooltips::set_tags(&enchantment_tags(&[], &[]));
    assert_kept_equals_rebuilt(&world, &stacks, &english, "first build");

    // Flags and text: other keys of the store, nothing to report.
    let armor = stack_named(&stacks, "dyed and damaged armor");
    let (normal, advanced) =
        (kept(&world, armor, &english, (false, false)), kept(&world, armor, &english, (true, false)));
    assert!(
        text_of(&advanced).iter().any(|l| l.starts_with("Durability: ")),
        "advanced lines: {:?}",
        text_of(&advanced)
    );
    assert!(!text_of(&normal).iter().any(|l| l.starts_with("Durability: ")));
    let painting = stack_named(&stacks, "painting without a variant");
    assert!(text_of(&kept(&world, painting, &english, (false, true))).contains(&"Random variant".to_owned()));
    assert!(!text_of(&kept(&world, painting, &english, (false, false))).contains(&"Random variant".to_owned()));
    let sword = stack_named(&stacks, "sword with default attributes");
    let french = Text { lang: &world.french, widths: FixedWidths { space: 4.0 }, font_generation: 1 };
    assert_eq!(text_of(&kept(&world, sword, &english, (false, false)))[0], "Diamond Sword");
    assert_eq!(text_of(&kept(&world, sword, &french, (false, false)))[0], "Épée en diamant");
    assert_kept_equals_rebuilt(&world, &stacks, &french, "language change");
    // Other fonts: same lines, other widths.
    let wide = Text { lang: &world.english, widths: FixedWidths { space: 9.0 }, font_generation: 2 };
    let map = stack_named(&stacks, "filled map");
    assert_ne!(kept(&world, map, &english, (false, false)).width, kept(&world, map, &wide, (false, false)).width);
    assert_kept_equals_rebuilt(&world, &stacks, &wide, "font change");

    // The tables read behind the scenes, each through the function the client calls.
    let plain = (false, false);
    assert_change_is_followed(&world, &stacks, &english, ("filled map", plain), "map data received", || {
        assert_eq!(text_of(&kept(&world, stack_named(&stacks, "filled map"), &english, plain))[1], "Unknown Map");
        crate::maps::store().apply(map_packet(MAP_ID));
    });
    assert_eq!(text_of(&kept(&world, stack_named(&stacks, "filled map"), &english, plain))[1], format!("Id #{MAP_ID}"));
    assert_change_is_followed(&world, &stacks, &english, ("spawn egg of a hostile mob", plain), "peaceful", || {
        crate::tooltips::set_context(true, false, 20.0);
    });
    let warned = ("egg carrying operator-only entity data", plain);
    assert_change_is_followed(&world, &stacks, &english, warned, "operator", || {
        crate::tooltips::set_context(true, true, 20.0);
    });
    assert_change_is_followed(&world, &stacks, &english, ("potion with a timed effect", plain), "tick rate", || {
        crate::tooltips::set_context(true, true, 10.0);
    });
    let enchanted = ("enchanted sword with name, lore and style", plain);
    assert_change_is_followed(&world, &stacks, &english, enchanted, "enchantment tags", || {
        // Sharpness (0) listed first, the curse (3) no longer a curse by tag.
        crate::tooltips::set_tags(&enchantment_tags(&[0, 3], &[1]));
    });
    let first_entry = |name: &str| vec![(name.to_owned(), None)];
    assert_change_is_followed(&world, &stacks, &english, ("goat horn", plain), "instrument registry", || {
        crate::tooltips_items::set_registry("minecraft:instrument", &first_entry("minecraft:ponder_goat_horn"));
    });
    assert_change_is_followed(&world, &stacks, &english, ("painting of a variant", plain), "painting registry", || {
        crate::tooltips_items::set_registry("minecraft:painting_variant", &first_entry("minecraft:kebab"));
    });
    let breaker = ("pickaxe that can break a block tag", plain);
    assert_change_is_followed(&world, &stacks, &english, breaker, "block tags", || {
        let oak_log = world.data.registry_id("minecraft:block", "minecraft:oak_log").expect("oak log") as i32;
        let breakable = vec![(BREAKABLE_TAG.to_owned(), vec![oak_log])];
        crate::tooltips_items::set_block_tags(&vec![("minecraft:block".to_owned(), breakable)]);
    });
    assert_writes_that_change_nothing_keep_the_tooltips();
    crate::tooltips::set_context(false, false, 20.0);
}

/// The context set again with the same values (every world tick) and the later packets of a
/// known map (colors, decorations) leave the kept tooltips valid; emptying a map table does not.
/// Other tests may write a tooltip table meanwhile: the count is bounded, not compared exactly.
fn assert_writes_that_change_nothing_keep_the_tooltips() {
    const WRITES: u64 = 10_000;
    let before = crate::tooltips::world_inputs_revision();
    for _ in 0..WRITES {
        crate::tooltips::set_context(true, true, 10.0);
        crate::maps::store().apply(map_packet(MAP_ID));
    }
    let moved = crate::tooltips::world_inputs_revision() - before;
    assert!(moved < WRITES, "{moved} reports for {WRITES} writes that change no tooltip");
    let before = crate::tooltips::world_inputs_revision();
    crate::maps::MapStore::default().clear();
    assert_ne!(crate::tooltips::world_inputs_revision(), before, "a cleared map table makes its maps unknown");
}

/// 300 frames hovering three stacks in turn while the context flips: every frame shows what a
/// rebuild gives, and a stack hovered again with nothing changed is not built again.
#[test]
fn alternating_hover_shows_rebuilt_tooltips_without_rebuilding() {
    let _one_at_a_time = world_inputs_lock();
    let world = TooltipWorld::new();
    let stacks = world.stacks();
    let text = Text { lang: &world.english, widths: FixedWidths { space: 4.0 }, font_generation: 1 };
    let hovered = ["spawn egg of a hostile mob", "potion with a timed effect", "written book"];
    crate::tooltips::set_context(false, false, 20.0);
    // Every tooltip object handed out, kept alive so that a new build never reuses an address.
    let mut handed: Vec<Rc<ItemTooltip>> = Vec::new();
    for frame in 0..300 {
        if frame % 75 == 0 {
            crate::tooltips::set_context(frame % 150 == 0, false, 20.0);
        }
        let stack = stack_named(&stacks, hovered[(frame / 5) % hovered.len()]);
        let tooltip = kept(&world, stack, &text, (false, false));
        assert_same_tooltip(&tooltip, &rebuilt(&world, stack, &text, (false, false)), &format!("frame {frame}"));
        if !handed.iter().any(|h| Rc::ptr_eq(h, &tooltip)) {
            handed.push(tooltip);
        }
    }
    // Four context values in turn, three stacks each: 12 builds. One build per change of hovered
    // stack would make 60, one per frame 300; another test writing a tooltip table adds a few.
    assert!(handed.len() < 30, "{} builds over 300 frames", handed.len());
    crate::tooltips::set_context(false, false, 20.0);
}

/// More stacks in turn than the store keeps: still what a rebuild gives.
#[test]
fn more_stacks_than_kept_slots() {
    let _one_at_a_time = world_inputs_lock();
    let world = TooltipWorld::new();
    let stacks = world.stacks();
    assert!(stacks.len() > KEPT_TOOLTIPS);
    let text = Text { lang: &world.english, widths: FixedWidths { space: 4.0 }, font_generation: 1 };
    for _round in 0..3 {
        for (label, stack) in &stacks {
            let advanced = (true, false);
            assert_same_tooltip(&kept(&world, stack, &text, advanced), &rebuilt(&world, stack, &text, advanced), label);
        }
    }
}

/// The enchantment name list is an input: the statistics screen passes none, and must not be
/// handed the tooltip built with the names (nor the reverse).
#[test]
fn enchantment_name_lists_are_told_apart() {
    let _one_at_a_time = world_inputs_lock();
    let world = TooltipWorld::new();
    let stacks = world.stacks();
    let stack = stack_named(&stacks, "enchanted sword with name, lore and style");
    let text = Text { lang: &world.english, widths: FixedWidths { space: 4.0 }, font_generation: 1 };
    let it = world.items();
    let named = kept(&world, stack, &text, (false, false));
    let request = Request { stack, it: &it, enchantments: &[], advanced: false, creative: false };
    let nameless = kept_or_built(&request, &text.source(), true);
    assert_same_tooltip(&nameless, &build(&request, &text.source()), "no enchantment names");
    assert_ne!(text_of(&named), text_of(&nameless), "the names give the enchantment lines");
    assert!(text_of(&named).contains(&"Sharpness V".to_owned()), "{:?}", text_of(&named));
}

/// Lines a screen adds: added once while the shared tooltip and the screen's inputs stay, again
/// when either changes.
#[test]
fn extended_tooltip_is_redone_only_on_change() {
    let widths = FixedWidths { space: 4.0 };
    let line = |text: &str| crate::gui::plain(text);
    let base = |text: &str| Rc::new(ItemTooltip { lines: vec![line(text)], width: 0, style: Some("fancy".into()) });
    let mut kept: Option<ExtendedTooltip<u8>> = None;
    let mut runs = 0;
    let mut refresh = |kept: &mut Option<ExtendedTooltip<u8>>, base: &Rc<ItemTooltip>, own: u8| {
        let tooltip = ExtendedTooltip::refresh(kept, Rc::clone(base), own, &widths, |lines| {
            runs += 1;
            lines.push(line("added line"));
        });
        (text_of(tooltip), tooltip.width, tooltip.style.clone())
    };
    let stone = base("Stone");
    let first = refresh(&mut kept, &stone, 0);
    assert_eq!(first, (vec!["Stone".to_owned(), "added line".to_owned()], 6 * 9 + 4, Some("fancy".to_owned())));
    assert_eq!(refresh(&mut kept, &stone, 0), first);
    let other_inputs = refresh(&mut kept, &stone, 1);
    // An equal tooltip rebuilt by the store is another object: redone.
    let rebuilt_stone = base("Stone");
    let other_base = refresh(&mut kept, &rebuilt_stone, 1);
    assert_eq!((other_inputs, other_base), (first.clone(), first));
    assert_eq!(runs, 3, "first, own inputs changed, shared tooltip replaced");
}

/// Micro-bench (ignored): `cargo test --release -p rc-app bench_hovered_tooltip -- --ignored
/// --nocapture`. What a hovered stack costs per frame: built again (the switch off, and what the
/// trading, crafting and dialog screens did) against kept. Fixed glyph widths stand for the fonts.
#[test]
#[ignore = "micro-bench"]
fn bench_hovered_tooltip() {
    const FRAMES: usize = 20_000;
    let _one_at_a_time = world_inputs_lock();
    let world = TooltipWorld::new();
    let text = Text { lang: &world.english, widths: FixedWidths { space: 4.0 }, font_generation: 1 };
    let it = world.items();
    let nanoseconds_per_frame = |frame: &dyn Fn()| {
        (0..5)
            .map(|_| {
                let start = std::time::Instant::now();
                (0..FRAMES).for_each(|_| frame());
                start.elapsed().as_nanos() as f64 / FRAMES as f64
            })
            .fold(f64::MAX, f64::min)
    };
    for (label, stack) in world.stacks() {
        let request =
            Request { stack: &stack, it: &it, enchantments: &world.enchantments, advanced: false, creative: false };
        let hover = |keep: bool| drop(std::hint::black_box(kept_or_built(&request, &text.source(), keep)));
        let (built, kept) = (nanoseconds_per_frame(&|| hover(false)), nanoseconds_per_frame(&|| hover(true)));
        println!("{label:<44} built {built:>8.0} ns per frame, kept {kept:>6.1} ns per frame");
    }
}
