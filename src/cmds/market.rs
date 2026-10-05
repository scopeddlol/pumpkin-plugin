//! Player market: list the stack in your hand for a price, browse and buy from a chest GUI.
//!
//! Only unmodified stacks can be listed (no enchantments, damage, custom names, container
//! contents, ...). A listing stores just the item key and count, so anything richer would be
//! silently lost or reset — refusing is safer than duping or destroying value.

use crate::{
    cmds::economy::{balance, deposit, ensure_account, money, withdraw},
    data::{self, MarketView, config, rt},
    tp::uuid_of,
    util::{
        Access, CmdResult, Cx, Cents, colored, command, fail, join, lit, now_secs, ok, parse_money, pretty_item, run, tell,
        warn, word,
    },
};
use pumpkin_plugin_api::{
    Context, Inventory, ItemStack, ItemStackExt, Player, Screen, Server,
    common::Hand,
    gui::Gui,
};
use serde::{Deserialize, Serialize};
use std::sync::{LazyLock, Mutex, MutexGuard};

const PAGE_SIZE: usize = 45;
const SLOT_PREV: u32 = 45;
const SLOT_INFO: u32 = 49;
const SLOT_NEXT: u32 = 53;

#[derive(Serialize, Deserialize, Clone)]
pub struct Listing {
    pub id: u64,
    pub seller: String,
    pub seller_name: String,
    pub item: String,
    pub count: u8,
    /// Total price for the whole stack.
    pub price: Cents,
    pub listed_at: u64,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Market {
    pub next_id: u64,
    pub listings: Vec<Listing>,
}

static MARKET: LazyLock<Mutex<Market>> = LazyLock::new(|| Mutex::new(Market::default()));

fn lock() -> MutexGuard<'static, Market> {
    MARKET.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn load() {
    *lock() = data::load_json("market.json");
}

fn mutate<R>(f: impl FnOnce(&mut Market) -> R) -> R {
    let mut guard = lock();
    let result = f(&mut guard);
    data::save_json("market.json", &*guard);
    result
}

fn snapshot() -> Vec<Listing> {
    lock().listings.clone()
}

pub fn register(ctx: &Context) {
    command(ctx, &["market", "mk"], "Browse and trade on the player market", "market", Access::Everyone, |c| {
        c.then(lit("sell").then(word("price").execute(run(sell))))
            .then(lit("mine").execute(run(mine)))
            .then(lit("cancel").then(word("id").execute(run(cancel))))
            .execute(run(open_cmd))
    });
}

// ---- helpers ----------------------------------------------------------------------------------

fn free_slot(player: &Player) -> Option<u32> {
    let inv = player.get_inventory().as_inventory();
    (0..36).find(|&i| inv.get_item(i).is_none())
}

fn give(player: &Player, item: &str, count: u8) -> bool {
    free_slot(player).is_some_and(|slot| {
        player
            .get_inventory()
            .as_inventory()
            .set_item(slot, Some(ItemStack::of(item, count)));
        true
    })
}

fn is_plain(stack: &ItemStack) -> bool {
    stack.get_components().is_empty()
        && stack.get_enchantments().is_empty()
        && stack.get_custom_enchantments().is_empty()
}

// ---- commands ---------------------------------------------------------------------------------

fn sell(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let raw = cx.require("price", "/market sell <price>")?;
    let price = parse_money(&raw).filter(|p| *p > 0).ok_or_else(|| fail("Enter a valid price greater than zero."))?;
    let id = uuid_of(&player);

    let held = player
        .get_item_in_hand(Hand::Right)
        .ok_or_else(|| fail("Hold the item stack you want to sell."))?;
    if !is_plain(&held) {
        return Err(fail("Enchanted, damaged, renamed or otherwise modified items can't be sold on the market."));
    }
    let max = config().market_max_listings;
    let mine = lock().listings.iter().filter(|l| l.seller == id).count();
    if mine >= max {
        return Err(fail(&format!("You can only have {max} active listings. Use /market cancel <id>.")));
    }

    let (key, count) = (held.get_registry_key(), held.get_count());
    // Take the item first; if anything below fails we haven't created value from nothing.
    player.set_item_in_hand(Hand::Right, None);
    let listing_id = mutate(|m| {
        m.next_id += 1;
        let listing = Listing {
            id: m.next_id,
            seller: id.clone(),
            seller_name: player.get_name(),
            item: key.clone(),
            count,
            price,
            listed_at: now_secs(),
        };
        m.listings.push(listing);
        m.next_id
    });
    cx.ok(&format!(
        "Listed &f{count}x {}&a for &e{}&a &8(id #{listing_id})&a.",
        pretty_item(&key),
        money(price)
    ))
}

fn mine(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let id = uuid_of(&player);
    let mine: Vec<Listing> = snapshot().into_iter().filter(|l| l.seller == id).collect();
    if mine.is_empty() {
        return cx.info("You have no active listings. Hold an item and use &e/market sell <price>&7.");
    }
    cx.reply(colored("&8&m      &r &6&lYour listings &8&m      "));
    for l in mine {
        cx.reply(join(vec![
            colored(&format!(
                " &8#{} &f{}x {} &8- &a{} ",
                l.id,
                l.count,
                pretty_item(&l.item),
                money(l.price)
            )),
            colored("&c[cancel]")
                .click_run_command(&format!("/market cancel {}", l.id))
                .hover_show_text(colored("&7Take this listing down")),
        ]));
    }
    Ok(1)
}

fn cancel(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let raw = cx.require("id", "/market cancel <id>")?;
    let listing_id: u64 = raw.trim_start_matches('#').parse().map_err(|_| fail("The id must be a number."))?;
    let me = uuid_of(&player);
    let admin = cx.has_perm("market.admin");

    let listing = lock()
        .listings
        .iter()
        .find(|l| l.id == listing_id && (l.seller == me || admin))
        .cloned()
        .ok_or_else(|| fail("You have no listing with that id."))?;
    if !give(&player, &listing.item, listing.count) {
        return Err(fail("Your inventory is full — free a slot first."));
    }
    mutate(|m| m.listings.retain(|l| l.id != listing_id));
    cx.ok(&format!("Returned &f{}x {}&a to you.", listing.count, pretty_item(&listing.item)))
}

fn open_cmd(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    open(&player, 0);
    Ok(1)
}

// ---- GUI --------------------------------------------------------------------------------------

fn pane(name: &str, color: &str) -> ItemStack {
    let stack = ItemStack::of(color, 1);
    stack.set_custom_name(Some(colored(name)));
    stack
}

fn render(gui: &Inventory, page: usize) -> (usize, Vec<u64>) {
    let listings = snapshot();
    let pages = listings.len().div_ceil(PAGE_SIZE).max(1);
    let page = page.min(pages - 1);
    gui.clear();

    let mut ids = Vec::new();
    for (slot, l) in listings.iter().skip(page * PAGE_SIZE).take(PAGE_SIZE).enumerate() {
        let stack = ItemStack::of(l.item.as_str(), l.count);
        stack.add_lore(colored(" "));
        stack.add_lore(colored(&format!("&7Price: &a{}", money(l.price))));
        stack.add_lore(colored(&format!("&7Seller: &f{}", l.seller_name)));
        stack.add_lore(colored(&format!("&8Listing #{}", l.id)));
        stack.add_lore(colored(" "));
        stack.add_lore(colored("&eClick to buy"));
        gui.set_item(slot as u32, Some(stack));
        ids.push(l.id);
    }
    for slot in 45..54 {
        gui.set_item(slot, Some(pane(" ", "gray_stained_glass_pane")));
    }
    if page > 0 {
        gui.set_item(SLOT_PREV, Some(pane("&e« Previous page", "arrow")));
    }
    if page + 1 < pages {
        gui.set_item(SLOT_NEXT, Some(pane("&eNext page »", "arrow")));
    }
    let info = pane(&format!("&6&lPlayer Market &8(&f{}&8/&f{pages}&8)", page + 1), "emerald");
    info.add_lore(colored(&format!("&7{} listing(s)", listings.len())));
    info.add_lore(colored("&7Sell: &e/market sell <price>"));
    info.add_lore(colored("&7Yours: &e/market mine"));
    gui.set_item(SLOT_INFO, Some(info));
    (page, ids)
}

pub fn open(player: &Player, page: usize) {
    let gui = Gui::new(Screen::Generic9x6, colored("&6&lMarket"));
    gui.set_allow_grab_items(false);
    gui.set_allow_put_items(false);
    // Keep a handle on the window's inventory: opening the GUI consumes the `Gui` itself.
    let inv = gui.get_inventory();
    let (page, ids) = render(&inv, page);
    player.open_gui(gui);
    rt(|r| r.market_open.insert(uuid_of(player), MarketView { page, ids, inv }));
}

/// Re-render the window the player already has open (page change, after a purchase, ...).
fn refresh(player: &Player, page: usize) {
    let id = uuid_of(player);
    match rt(|r| r.market_open.remove(&id)) {
        Some(mut view) => {
            let (page, ids) = render(&view.inv, page);
            view.page = page;
            view.ids = ids;
            rt(|r| r.market_open.insert(id, view));
        }
        None => open(player, page),
    }
}

pub fn is_viewing(player: &Player) -> bool {
    rt(|r| r.market_open.contains_key(&uuid_of(player)))
}

pub fn on_close(player: &Player) {
    rt(|r| r.market_open.remove(&uuid_of(player)));
}

/// Handle a click in the market window. Returns after cancelling is the caller's job.
pub fn on_click(server: &Server, player: &Player, raw_slot: i16) {
    let Ok(slot) = u32::try_from(raw_slot) else {
        return;
    };
    let id = uuid_of(player);
    let Some((page, ids)) = rt(|r| r.market_open.get(&id).map(|v| (v.page, v.ids.clone()))) else {
        return;
    };

    match slot {
        SLOT_PREV if page > 0 => refresh(player, page - 1),
        SLOT_NEXT => refresh(player, page + 1),
        s if (s as usize) < PAGE_SIZE => {
            if let Some(listing_id) = ids.get(s as usize).copied() {
                buy(server, player, listing_id, page);
            }
        }
        _ => {}
    }
}

fn buy(server: &Server, player: &Player, listing_id: u64, page: usize) {
    let me = uuid_of(player);
    let Some(listing) = lock().listings.iter().find(|l| l.id == listing_id).cloned() else {
        tell(player, warn("That listing was just sold or removed."));
        refresh(player, page);
        return;
    };
    if listing.seller == me {
        tell(player, warn("That's your own listing — use &e/market cancel <id>&7 to take it down."));
        return;
    }
    if free_slot(player).is_none() {
        tell(player, crate::util::bad("Your inventory is full."));
        return;
    }
    if !withdraw(&me, listing.price) {
        tell(
            player,
            crate::util::bad(&format!("You need {} (you have {}).", money(listing.price), money(balance(&me)))),
        );
        return;
    }
    // Remove the listing before handing over the item so a double-click can't buy it twice.
    let removed = mutate(|m| {
        let before = m.listings.len();
        m.listings.retain(|l| l.id != listing_id);
        before != m.listings.len()
    });
    if !removed {
        deposit(&me, listing.price);
        refresh(player, page);
        return;
    }
    give(player, &listing.item, listing.count);

    let tax_pct = config().market_tax_percent.clamp(0.0, 100.0);
    let proceeds = listing.price - ((listing.price as f64) * tax_pct / 100.0).round() as Cents;
    ensure_account(&listing.seller, &listing.seller_name);
    deposit(&listing.seller, proceeds);

    tell(
        player,
        ok(&format!("Bought &f{}x {}&a for &e{}&a.", listing.count, pretty_item(&listing.item), money(listing.price))),
    );
    if let Some(seller) = server.get_all_players().into_iter().find(|p| uuid_of(p) == listing.seller) {
        tell(
            &seller,
            ok(&format!(
                "&e{}&a bought your &f{}x {}&a — you earned &e{}&a.",
                player.get_name(),
                listing.count,
                pretty_item(&listing.item),
                money(proceeds)
            )),
        );
    }
    refresh(player, page);
}
