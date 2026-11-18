//! Item tooltips kept between frames, one store for every screen.
//!
//! The game builds the tooltip of the hovered stack again at every frame. Here the built tooltip
//! is handed back as long as a rebuild would give the same one: same stack and same [`Inputs`]
//! (everything else the lines, their width and their style are made from). Whatever changes a line
//! must therefore change the stack or an input; the tables read behind the scenes report their
//! writes through [`crate::tooltips::world_inputs_changed`].
//!
//! `RUSTCRAFT_OPT_SHARED_TOOLTIP_CACHE=0` rebuilds at every call, like the game. The caches each
//! screen held before compared the stack alone and went stale (a map whose data arrived while
//! hovered, a difficulty or operator change, a font change): they are gone on both paths.

use std::cell::RefCell;
use std::rc::Rc;

use rc_proto_v777::item::ItemStack;
use rc_text::StyledChar;

use crate::gui::Gui;
use crate::hud::HotbarItems;

/// Tooltips kept at once: the hovered stack, plus the few a screen alternates between (ghost
/// recipe over a filled slot).
const KEPT_TOOLTIPS: usize = 4;

fn enabled() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_SHARED_TOOLTIP_CACHE")
}

/// An item tooltip ready to draw.
#[derive(Debug, PartialEq)]
pub(crate) struct ItemTooltip {
    pub lines: Vec<Vec<StyledChar>>,
    /// Width of the widest line.
    pub width: i32,
    /// Resource id of the stack's `tooltip_style` component (background and frame sprites).
    pub style: Option<String>,
}

impl ItemTooltip {
    fn of_lines(lines: Vec<Vec<StyledChar>>, style: Option<String>, metrics: &dyn rc_text::Metrics) -> ItemTooltip {
        let width = lines.iter().map(|line| rc_text::width(metrics, line)).max().unwrap_or(0);
        ItemTooltip { lines, width, style }
    }
}

/// The interface as a tooltip reads it: translations and glyph widths.
pub(crate) struct TextSource<'a> {
    pub lang: &'a rc_text::Lang,
    pub metrics: &'a dyn rc_text::Metrics,
    /// Identity of the fonts behind `metrics`: moves whenever the widths may have.
    pub font_generation: usize,
}

impl<'a> TextSource<'a> {
    pub(crate) fn of(gui: &'a Gui) -> TextSource<'a> {
        TextSource { lang: &gui.lang, metrics: gui, font_generation: gui.font_generation() }
    }
}

/// The stack whose tooltip is asked, with the tables and flags its lines are built from.
pub(crate) struct Request<'a> {
    pub stack: &'a ItemStack,
    pub it: &'a HotbarItems<'a>,
    /// Names of the enchantment registry received from the server, in id order.
    pub enchantments: &'a [String],
    /// Advanced tooltips (F3 + H).
    pub advanced: bool,
    /// Tooltip of the creative inventory (random painting, effects of a suspicious stew).
    pub creative: bool,
}

/// What a tooltip is made from besides its stack. Two equal values give the same tooltip.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Inputs {
    /// Tooltip context, tags, synchronized registries and known maps
    /// (`tooltips::world_inputs_revision`).
    world: u64,
    /// Identity of the translation table.
    language: u64,
    /// Identity of the fonts (line widths).
    fonts: usize,
    /// Bound keys (`keybind` components of a name or a lore line).
    keybinds: u64,
    advanced: bool,
    creative: bool,
    /// Address and length of the enchantment name list: tells the callers' lists apart (the
    /// statistics screen passes none). A list replaced at the same address reports it as a
    /// world input.
    enchantment_names: (usize, usize),
    /// Address of the version data the item tables come from (one per process outside tests).
    version_data: usize,
}

impl Inputs {
    fn of(request: &Request<'_>, text: &TextSource<'_>) -> Inputs {
        Inputs {
            world: crate::tooltips::world_inputs_revision(),
            language: text.lang.generation,
            fonts: text.font_generation,
            keybinds: rc_text::keybinds_generation(),
            advanced: request.advanced,
            creative: request.creative,
            enchantment_names: (request.enchantments.as_ptr() as usize, request.enchantments.len()),
            version_data: std::ptr::from_ref(request.it.data) as usize,
        }
    }
}

struct Kept {
    stack: ItemStack,
    inputs: Inputs,
    tooltip: Rc<ItemTooltip>,
}

thread_local! {
    /// Tooltips kept by this thread (one thread draws the interface), oldest first.
    static KEPT: RefCell<Vec<Kept>> = const { RefCell::new(Vec::new()) };
}

/// Tooltip of `stack`, equal to the one built now: kept from an earlier call when the stack and
/// every input are unchanged, built otherwise. `creative`: tooltip of the creative inventory.
pub(crate) fn item_tooltip(
    stack: &ItemStack,
    it: &HotbarItems<'_>,
    enchantments: &[String],
    gui: &Gui,
    advanced: bool,
    creative: bool,
) -> Rc<ItemTooltip> {
    kept_or_built(&Request { stack, it, enchantments, advanced, creative }, &TextSource::of(gui), enabled())
}

/// [`item_tooltip`] from any text source; `keep` false builds at every call.
fn kept_or_built(request: &Request<'_>, text: &TextSource<'_>, keep: bool) -> Rc<ItemTooltip> {
    if !keep {
        return Rc::new(build(request, text));
    }
    let inputs = Inputs::of(request, text);
    let kept = KEPT.with_borrow(|kept| {
        // Newest first: the stack hovered last frame is nearly always the one asked. The inputs
        // before the stack: a few integers, against the bytes of every component.
        let found = kept.iter().rev().find(|k| k.inputs == inputs && k.stack == *request.stack);
        found.map(|k| Rc::clone(&k.tooltip))
    });
    if let Some(tooltip) = kept {
        return tooltip;
    }
    let tooltip = Rc::new(build(request, text));
    KEPT.with_borrow_mut(|kept| {
        if kept.len() == KEPT_TOOLTIPS {
            kept.remove(0);
        }
        kept.push(Kept { stack: request.stack.clone(), inputs, tooltip: Rc::clone(&tooltip) });
    });
    tooltip
}

/// The tooltip as the game builds it at every frame: lines, width, style.
fn build(request: &Request<'_>, text: &TextSource<'_>) -> ItemTooltip {
    let &Request { stack, it, enchantments, advanced, creative } = request;
    // The line builders read the creative flag of their thread.
    crate::tooltips_more::set_creative(creative);
    let lines = crate::screens::tooltip_lines(stack, it, enchantments, text.lang, advanced);
    crate::tooltips_more::set_creative(false);
    let info = usize::try_from(stack.item).ok().and_then(|i| it.infos.get(i));
    let view = crate::stack::StackRef { stack, info, ids: it.ids, data: it.data, hold: crate::stack::Hold::NONE };
    ItemTooltip::of_lines(lines, crate::screens::tooltip_style(&view), text.metrics)
}

/// An item tooltip to which a screen added lines of its own (tab names of the creative inventory,
/// "more recipes" of the recipe book), kept while the shared tooltip it starts from is still the
/// current one and the screen's own inputs are unchanged.
pub(crate) struct ExtendedTooltip<OwnInputs> {
    base: Rc<ItemTooltip>,
    own_inputs: OwnInputs,
    extended: ItemTooltip,
}

impl<OwnInputs: PartialEq> ExtendedTooltip<OwnInputs> {
    /// `base` with the lines `extend` adds; `extend` runs only when `base` or `own_inputs`
    /// changed since the tooltip in `kept` was made.
    pub(crate) fn refresh<'a>(
        kept: &'a mut Option<Self>,
        base: Rc<ItemTooltip>,
        own_inputs: OwnInputs,
        metrics: &dyn rc_text::Metrics,
        extend: impl FnOnce(&mut Vec<Vec<StyledChar>>),
    ) -> &'a ItemTooltip {
        // The same shared tooltip object means the same stack and inputs: the store hands a
        // new object after every rebuild, and this holder keeps the old one alive meanwhile.
        let fresh = kept.as_ref().is_some_and(|k| Rc::ptr_eq(&k.base, &base) && k.own_inputs == own_inputs);
        if !fresh {
            *kept = None;
        }
        let kept = kept.get_or_insert_with(|| {
            let mut lines = base.lines.clone();
            extend(&mut lines);
            let extended = ItemTooltip::of_lines(lines, base.style.clone(), metrics);
            ExtendedTooltip { base, own_inputs, extended }
        });
        &kept.extended
    }
}

#[cfg(test)]
mod tests;
