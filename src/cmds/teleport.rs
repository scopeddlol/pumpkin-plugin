//! Homes, warps, spawn, /back, /rtp and the /tpa family.

use crate::{
    data::{self, Loc, TpaRequest, config, mutate, rt},
    perms,
    tp::{self, fmt_loc, loc_of, uuid_of},
    util::{
        Access, CmdResult, Cx, colored, command, join, fail, find_online, fmt_duration, now_secs, ok, player_arg,
        register_node, run, suggestions, tell, word,
    },
};
use pumpkin_plugin_api::{
    Context, Player, Server,
    command::{CommandSender, CommandSuggestions, SuggestionRequest},
    commands::CommandSuggestionHandler,
    permission::PermissionLevel,
    text::TextComponent,
};

struct HomeNames;
impl CommandSuggestionHandler for HomeNames {
    fn suggest(&self, sender: CommandSender, _s: Server, req: SuggestionRequest) -> CommandSuggestions {
        let prefix = req.remaining.to_lowercase();
        let names = sender
            .as_player()
            .map(|p| data::with(|d| d.users.get(&uuid_of(&p)).map(|u| u.homes.keys().cloned().collect::<Vec<_>>())))
            .flatten()
            .unwrap_or_default();
        suggestions(&req, names.into_iter().filter(|n| n.starts_with(&prefix)).collect())
    }
}

struct WarpNames;
impl CommandSuggestionHandler for WarpNames {
    fn suggest(&self, _sender: CommandSender, _s: Server, req: SuggestionRequest) -> CommandSuggestions {
        let prefix = req.remaining.to_lowercase();
        let names = data::with(|d| d.warps.keys().cloned().collect::<Vec<_>>());
        suggestions(&req, names.into_iter().filter(|n| n.starts_with(&prefix)).collect())
    }
}

pub fn register(ctx: &Context) {
    let op = Access::Op(PermissionLevel::Two);
    let all = Access::Everyone;

    register_node(ctx, "tp.bypass", "Skip teleport warm-ups and cooldowns", op);

    command(ctx, &["spawn"], "Teleport to the server spawn", "spawn", all, |c| c.execute(run(spawn)));
    command(ctx, &["setspawn"], "Set the server spawn here", "setspawn", op, |c| c.execute(run(setspawn)));

    command(ctx, &["sethome"], "Set a home here", "sethome", all, |c| {
        c.then(word("name").execute(run(sethome))).execute(run(sethome))
    });
    command(ctx, &["home"], "Teleport to a home", "home", all, |c| {
        c.then(word("name").suggest(HomeNames).execute(run(home))).execute(run(home))
    });
    command(ctx, &["delhome"], "Delete a home", "delhome", all, |c| {
        c.then(word("name").suggest(HomeNames).execute(run(delhome)))
    });
    command(ctx, &["homes"], "List your homes", "homes", all, |c| c.execute(run(homes)));

    command(ctx, &["setwarp"], "Create a warp here", "setwarp", op, |c| c.then(word("name").execute(run(setwarp))));
    command(ctx, &["delwarp"], "Delete a warp", "delwarp", op, |c| {
        c.then(word("name").suggest(WarpNames).execute(run(delwarp)))
    });
    command(ctx, &["warp"], "Teleport to a warp", "warp", all, |c| {
        c.then(word("name").suggest(WarpNames).execute(run(warp))).execute(run(warps))
    });
    command(ctx, &["warps"], "List all warps", "warps", all, |c| c.execute(run(warps)));

    command(ctx, &["back"], "Return to your previous location", "back", all, |c| c.execute(run(back)));
    command(ctx, &["rtp", "wild"], "Teleport to a random safe location", "rtp", all, |c| c.execute(run(rtp)));

    command(ctx, &["tpa"], "Ask to teleport to a player", "tpa", all, |c| {
        c.then(player_arg("player").execute(run(|cx| request(cx, true))))
    });
    command(ctx, &["tpahere"], "Ask a player to teleport to you", "tpahere", all, |c| {
        c.then(player_arg("player").execute(run(|cx| request(cx, false))))
    });
    command(ctx, &["tpaccept", "tpyes"], "Accept a teleport request", "tpaccept", all, |c| {
        c.then(player_arg("player").execute(run(tpaccept))).execute(run(tpaccept))
    });
    command(ctx, &["tpdeny", "tpno"], "Deny a teleport request", "tpdeny", all, |c| {
        c.then(player_arg("player").execute(run(tpdeny))).execute(run(tpdeny))
    });
    command(ctx, &["tpcancel"], "Cancel your outgoing teleport request", "tpcancel", all, |c| {
        c.execute(run(tpcancel))
    });
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 24 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn norm_name(raw: Option<String>) -> String {
    raw.unwrap_or_else(|| "home".into()).to_lowercase()
}

fn go(cx: &Cx, player: &Player, dest: Loc, label: &str) -> CmdResult {
    tp::teleport_with_warmup(&cx.server, player, dest, label).map_err(|e| fail(&e))?;
    Ok(1)
}

// ---- spawn ------------------------------------------------------------------------------------

fn spawn(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let dest = tp::spawn_loc(&cx.server).ok_or_else(|| fail("No spawn is available."))?;
    go(cx, &player, dest, "spawn")
}

fn setspawn(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let loc = loc_of(&player);
    let desc = fmt_loc(&loc);
    mutate(|d| d.spawn = Some(loc));
    cx.ok(&format!("Spawn set to &e{desc}&a."))
}

// ---- homes ------------------------------------------------------------------------------------

fn sethome(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let name = norm_name(cx.arg("name"));
    if !valid_name(&name) {
        return Err(fail("Home names may use letters, numbers, - and _ (max 24)."));
    }
    let id = uuid_of(&player);
    let limit = perms::homes_limit(&id, config().default_homes) as usize;
    let loc = loc_of(&player);
    let outcome = mutate(|d| {
        let user = d.users.entry(id.clone()).or_default();
        if !user.homes.contains_key(&name) && user.homes.len() >= limit {
            return Err(limit);
        }
        user.homes.insert(name.clone(), loc);
        Ok(())
    });
    match outcome {
        Ok(()) => cx.ok(&format!("Home &e{name}&a set.")),
        Err(limit) => Err(fail(&format!("You've reached your limit of {limit} homes. Delete one with /delhome."))),
    }
}

fn home(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let id = uuid_of(&player);
    let homes = data::with(|d| d.users.get(&id).map(|u| u.homes.clone()).unwrap_or_default());
    let name = match cx.arg("name") {
        Some(n) => n.to_lowercase(),
        None if homes.len() == 1 => homes.keys().next().cloned().unwrap_or_default(),
        None if homes.contains_key("home") => "home".into(),
        None if homes.is_empty() => return Err(fail("You have no homes. Use /sethome first.")),
        None => return homes_list(cx, &homes).map(|()| 1),
    };
    let dest = homes
        .get(&name)
        .cloned()
        .ok_or_else(|| fail(&format!("You don't have a home called '{name}'.")))?;
    go(cx, &player, dest, &format!("home {name}"))
}

fn delhome(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let name = cx.require("name", "/delhome <name>")?.to_lowercase();
    let id = uuid_of(&player);
    let removed = mutate(|d| d.users.get_mut(&id).and_then(|u| u.homes.remove(&name)));
    if removed.is_some() {
        cx.ok(&format!("Home &e{name}&a deleted."))
    } else {
        Err(fail(&format!("You don't have a home called '{name}'.")))
    }
}

fn homes_list(cx: &Cx, homes: &std::collections::BTreeMap<String, Loc>) -> Result<(), pumpkin_plugin_api::command::CommandError> {
    let limit = cx
        .sender
        .as_player()
        .map_or(0, |p| perms::homes_limit(&uuid_of(&p), config().default_homes));
    let mut parts = vec![colored(&format!("&8&l» &7Homes &8(&f{}&8/&f{limit}&8)&7: ", homes.len()))];
    for (i, name) in homes.keys().enumerate() {
        if i > 0 {
            parts.push(colored("&8, "));
        }
        parts.push(button(&format!("&e{name}"), &format!("/home {name}"), "&7Click to teleport"));
    }
    cx.reply(join(parts));
    Ok(())
}

fn homes(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let id = uuid_of(&player);
    let homes = data::with(|d| d.users.get(&id).map(|u| u.homes.clone()).unwrap_or_default());
    if homes.is_empty() {
        return cx.info("You have no homes yet. Use &e/sethome&7 to create one.");
    }
    homes_list(cx, &homes)?;
    Ok(1)
}

// ---- warps ------------------------------------------------------------------------------------

fn setwarp(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let name = cx.require("name", "/setwarp <name>")?.to_lowercase();
    if !valid_name(&name) {
        return Err(fail("Warp names may use letters, numbers, - and _ (max 24)."));
    }
    let loc = loc_of(&player);
    mutate(|d| d.warps.insert(name.clone(), loc));
    cx.ok(&format!("Warp &e{name}&a saved."))
}

fn delwarp(cx: &Cx) -> CmdResult {
    let name = cx.require("name", "/delwarp <name>")?.to_lowercase();
    if mutate(|d| d.warps.remove(&name)).is_some() {
        cx.ok(&format!("Warp &e{name}&a deleted."))
    } else {
        Err(fail(&format!("There is no warp called '{name}'.")))
    }
}

fn warp(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let name = cx.require("name", "/warp <name>")?.to_lowercase();
    let dest = data::with(|d| d.warps.get(&name).cloned())
        .ok_or_else(|| fail(&format!("There is no warp called '{name}'.")))?;
    go(cx, &player, dest, &format!("warp {name}"))
}

fn warps(cx: &Cx) -> CmdResult {
    let names = data::with(|d| d.warps.keys().cloned().collect::<Vec<_>>());
    if names.is_empty() {
        return cx.info("There are no warps yet.");
    }
    let mut parts = vec![colored(&format!("&8&l» &7Warps &8(&f{}&8)&7: ", names.len()))];
    for (i, name) in names.iter().enumerate() {
        if i > 0 {
            parts.push(colored("&8, "));
        }
        parts.push(button(&format!("&b{name}"), &format!("/warp {name}"), "&7Click to warp"));
    }
    cx.reply(join(parts));
    Ok(1)
}

// ---- back / rtp -------------------------------------------------------------------------------

fn back(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let dest = rt(|r| r.back.get(&uuid_of(&player)).cloned()).ok_or_else(|| fail("You have nowhere to go back to."))?;
    go(cx, &player, dest, "your previous location")
}

fn rtp(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let id = uuid_of(&player);
    let left = tp::cooldown_left(&id, "rtp");
    if left > 0 && !cx.has_perm("tp.bypass") {
        return Err(fail(&format!("You can use /rtp again in {}.", fmt_duration(left))));
    }
    cx.info("Scouting for a safe spot...")?;
    let dest = tp::find_random_spot(&cx.server, &player)
        .ok_or_else(|| fail("Couldn't find a safe spot (random teleport only works in the overworld). Try again."))?;
    let label = format!("{:.0}, {:.0}", dest.x, dest.z);
    go(cx, &player, dest, &label)?;
    tp::start_cooldown(&id, "rtp", config().rtp_cooldown_secs);
    Ok(1)
}

// ---- tpa --------------------------------------------------------------------------------------

fn button(label: &str, command: &str, hover: &str) -> TextComponent {
    colored(label).click_run_command(command).hover_show_text(colored(hover))
}

fn request(cx: &Cx, from_goes_to_target: bool) -> CmdResult {
    let me = cx.player()?;
    let name = cx.require("player", "/tpa <player>")?;
    let target = cx.online(&name)?;
    if uuid_of(&target) == uuid_of(&me) {
        return Err(fail("You can't send a request to yourself."));
    }
    tp::prune_requests();
    let cfg = config();
    let (from, to) = (me.get_name(), target.get_name());
    rt(|r| {
        r.tpa.retain(|t| !(t.from == from && t.to == to));
        r.tpa.push(TpaRequest {
            from: from.clone(),
            to: to.clone(),
            from_goes_to_target,
            expires: now_secs() + cfg.tpa_timeout_secs,
        });
    });

    let what = if from_goes_to_target { "to teleport to you" } else { "you to teleport to them" };
    let line = join(vec![
        colored(&format!("&8&l» &e{from} &7wants {what}. ")),
        button("&a&l[ACCEPT]", &format!("/tpaccept {from}"), "&7Click to accept"),
        colored(" "),
        button("&c&l[DENY]", &format!("/tpdeny {from}"), "&7Click to deny"),
    ]);
    tell(&target, line);
    cx.ok(&format!("Request sent to &e{to}&a. It expires in &f{}&a.", fmt_duration(cfg.tpa_timeout_secs)))
}

/// Takes the newest pending request addressed to `me` (optionally from `only_from`).
fn take_request(me: &Player, only_from: Option<&str>) -> Option<TpaRequest> {
    tp::prune_requests();
    let my_name = me.get_name();
    rt(|r| {
        let idx = r
            .tpa
            .iter()
            .rposition(|t| t.to == my_name && only_from.is_none_or(|f| t.from.eq_ignore_ascii_case(f)))?;
        Some(r.tpa.remove(idx))
    })
}

fn tpaccept(cx: &Cx) -> CmdResult {
    let me = cx.player()?;
    let req = take_request(&me, cx.arg("player").as_deref()).ok_or_else(|| fail("You have no pending teleport requests."))?;
    let from = find_online(&cx.server, &req.from).ok_or_else(|| fail(&format!("{} is no longer online.", req.from)))?;
    let (mover, anchor) = if req.from_goes_to_target { (&from, &me) } else { (&me, &from) };
    tell(anchor, ok(&format!("Accepted. &e{}&a is on the way.", mover.get_name())));
    tell(mover, ok(&format!("&e{}&a accepted your request.", anchor.get_name())));
    let dest = loc_of(anchor);
    tp::teleport_with_warmup(&cx.server, mover, dest, &anchor.get_name()).map_err(|e| fail(&e))?;
    Ok(1)
}

fn tpdeny(cx: &Cx) -> CmdResult {
    let me = cx.player()?;
    let req = take_request(&me, cx.arg("player").as_deref()).ok_or_else(|| fail("You have no pending teleport requests."))?;
    if let Some(from) = find_online(&cx.server, &req.from) {
        tell(&from, crate::util::bad(&format!("{} denied your teleport request.", me.get_name())));
    }
    cx.info(&format!("Denied the request from &e{}&7.", req.from))
}

fn tpcancel(cx: &Cx) -> CmdResult {
    let me = cx.player()?;
    tp::prune_requests();
    let name = me.get_name();
    let removed = rt(|r| {
        let before = r.tpa.len();
        r.tpa.retain(|t| t.from != name);
        before - r.tpa.len()
    });
    if removed == 0 {
        return Err(fail("You have no outgoing requests."));
    }
    cx.info("Your teleport request was cancelled.")
}
