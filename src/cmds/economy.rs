//! Economy: balances, payments, leaderboard and admin /eco.

use crate::{
    data::{self, config, mutate},
    tp::uuid_of,
    util::{
        Access, CmdResult, Cx, Cents, colored, command, fail, find_online, fmt_amount, lit, parse_money, player_arg,
        run, tell, ok, word,
    },
};
use pumpkin_plugin_api::{Context, permission::PermissionLevel};

// ---- account API (shared with the market) -----------------------------------------------------

pub fn money(cents: Cents) -> String {
    format!("{}{}", config().currency_symbol, fmt_amount(cents))
}

/// Create the account with the starting balance on first sight.
pub fn ensure_account(uuid: &str, name: &str) {
    let start = (config().starting_balance * 100.0).round() as Cents;
    mutate(|d| {
        let user = d.users.entry(uuid.to_string()).or_insert_with(|| crate::data::User {
            balance: start,
            ..Default::default()
        });
        user.name = name.to_string();
    });
}

pub fn balance(uuid: &str) -> Cents {
    data::with(|d| d.users.get(uuid).map_or(0, |u| u.balance))
}

pub fn deposit(uuid: &str, cents: Cents) {
    mutate(|d| {
        let u = d.users.entry(uuid.to_string()).or_default();
        u.balance = u.balance.saturating_add(cents);
    });
}

/// Atomically take money; `false` (and no change) if the account can't afford it.
pub fn withdraw(uuid: &str, cents: Cents) -> bool {
    mutate(|d| {
        let u = d.users.entry(uuid.to_string()).or_default();
        if u.balance >= cents {
            u.balance -= cents;
            true
        } else {
            false
        }
    })
}

/// Resolve a (possibly offline) player by name to `(uuid, display name)`.
fn resolve(cx: &Cx, name: &str) -> Result<(String, String), pumpkin_plugin_api::command::CommandError> {
    if let Some(p) = find_online(&cx.server, name) {
        return Ok((uuid_of(&p), p.get_name()));
    }
    data::with(|d| d.find_user(name).map(|(id, u)| (id.clone(), u.name.clone())))
        .ok_or_else(|| fail(&format!("I don't know a player called '{name}'.")))
}

pub fn register(ctx: &Context) {
    let all = Access::Everyone;
    command(ctx, &["bal", "balance", "money"], "Check your (or another player's) balance", "bal", all, |c| {
        c.then(player_arg("player").execute(run(bal))).execute(run(bal))
    });
    command(ctx, &["pay"], "Pay another player", "pay", all, |c| {
        c.then(player_arg("player").then(word("amount").execute(run(pay))))
    });
    command(ctx, &["baltop", "balancetop"], "Richest players", "baltop", all, |c| {
        c.then(word("page").execute(run(baltop))).execute(run(baltop))
    });
    command(ctx, &["eco", "economy"], "Manage player balances", "eco", Access::Op(PermissionLevel::Three), |c| {
        let action = |name: &str, h: fn(&Cx) -> CmdResult| {
            lit(name).then(player_arg("player").then(word("amount").execute(run(h))))
        };
        c.then(action("give", eco_give))
            .then(action("take", eco_take))
            .then(action("set", eco_set))
            .then(lit("reset").then(player_arg("player").execute(run(eco_reset))))
    });
}

fn bal(cx: &Cx) -> CmdResult {
    match cx.arg("player") {
        Some(name) => {
            let (id, display) = resolve(cx, &name)?;
            cx.info(&format!("&f{display}&7's balance: &a{}", money(balance(&id))))
        }
        None => {
            let me = cx.player()?;
            cx.info(&format!("Your balance: &a{}", money(balance(&uuid_of(&me)))))
        }
    }
}

fn amount_arg(cx: &Cx, usage: &str, allow_zero: bool) -> Result<Cents, pumpkin_plugin_api::command::CommandError> {
    let raw = cx.require("amount", usage)?;
    let cents = parse_money(&raw).ok_or_else(|| fail("Enter a valid amount, e.g. 50, 12.5 or 2k."))?;
    if cents == 0 && !allow_zero {
        return Err(fail("The amount must be greater than zero."));
    }
    Ok(cents)
}

fn pay(cx: &Cx) -> CmdResult {
    let me = cx.player()?;
    let name = cx.require("player", "/pay <player> <amount>")?;
    let cents = amount_arg(cx, "/pay <player> <amount>", false)?;
    let (to_id, to_name) = resolve(cx, &name)?;
    let from_id = uuid_of(&me);
    if to_id == from_id {
        return Err(fail("You can't pay yourself."));
    }
    if !withdraw(&from_id, cents) {
        return Err(fail(&format!("You can't afford that. Balance: {}", money(balance(&from_id)))));
    }
    deposit(&to_id, cents);
    if let Some(target) = find_online(&cx.server, &to_name) {
        tell(&target, ok(&format!("You received &e{}&a from &e{}&a.", money(cents), me.get_name())));
    }
    cx.ok(&format!("Paid &e{}&a to &e{to_name}&a.", money(cents)))
}

fn baltop(cx: &Cx) -> CmdResult {
    let page = cx.arg("page").and_then(|p| p.parse::<usize>().ok()).unwrap_or(1).max(1);
    let mut rows: Vec<(String, Cents)> = data::with(|d| {
        d.users
            .values()
            .filter(|u| !u.name.is_empty())
            .map(|u| (u.name.clone(), u.balance))
            .collect()
    });
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let pages = rows.len().div_ceil(10).max(1);
    let page = page.min(pages);
    cx.reply(colored(&format!("&8&m      &r &6&lRichest players &8(&f{page}&8/&f{pages}&8) &8&m      ")));
    for (i, (name, cents)) in rows.iter().enumerate().skip((page - 1) * 10).take(10) {
        let rank = match i {
            0 => "&6&l1.".to_string(),
            1 => "&7&l2.".to_string(),
            2 => "&c&l3.".to_string(),
            n => format!("&8{}.", n + 1),
        };
        cx.reply(colored(&format!(" {rank} &f{name} &8- &a{}", money(*cents))));
    }
    Ok(1)
}

fn eco_target(cx: &Cx) -> Result<(String, String), pumpkin_plugin_api::command::CommandError> {
    let name = cx.require("player", "/eco <give|take|set|reset> <player> [amount]")?;
    resolve(cx, &name)
}

fn eco_give(cx: &Cx) -> CmdResult {
    let (id, name) = eco_target(cx)?;
    let cents = amount_arg(cx, "/eco give <player> <amount>", false)?;
    deposit(&id, cents);
    notify(cx, &id, &name, &format!("You received &e{}&a.", money(cents)));
    cx.ok(&format!("Gave &e{}&a to &e{name}&a. New balance: &f{}", money(cents), money(balance(&id))))
}

fn eco_take(cx: &Cx) -> CmdResult {
    let (id, name) = eco_target(cx)?;
    let cents = amount_arg(cx, "/eco take <player> <amount>", false)?;
    let taken = cents.min(balance(&id));
    withdraw(&id, taken);
    notify(cx, &id, &name, &format!("&e{}&a was taken from your balance.", money(taken)));
    cx.ok(&format!("Took &e{}&a from &e{name}&a. New balance: &f{}", money(taken), money(balance(&id))))
}

fn eco_set(cx: &Cx) -> CmdResult {
    let (id, name) = eco_target(cx)?;
    let cents = amount_arg(cx, "/eco set <player> <amount>", true)?;
    mutate(|d| d.users.entry(id.clone()).or_default().balance = cents);
    notify(cx, &id, &name, &format!("Your balance was set to &e{}&a.", money(cents)));
    cx.ok(&format!("Set &e{name}&a's balance to &f{}&a.", money(cents)))
}

fn eco_reset(cx: &Cx) -> CmdResult {
    let (id, name) = eco_target(cx)?;
    let start = (config().starting_balance * 100.0).round() as Cents;
    mutate(|d| d.users.entry(id.clone()).or_default().balance = start);
    notify(cx, &id, &name, &format!("Your balance was reset to &e{}&a.", money(start)));
    cx.ok(&format!("Reset &e{name}&a's balance to &f{}&a.", money(start)))
}

fn notify(cx: &Cx, _id: &str, name: &str, message: &str) {
    if let Some(p) = find_online(&cx.server, name) {
        tell(&p, ok(message));
    }
}
