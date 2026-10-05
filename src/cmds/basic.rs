//! Gamemodes, flight, speed, healing, god mode, private messages and other quality-of-life commands.

use crate::{
    data::{self, rt},
    tp::{self, uuid_of},
    util::{
        Access, Cx, CmdResult, bad, colored, command, fail, fmt_duration, greedy, info, now_secs, ok,
        player_arg, register_node, run, strip_codes, tell, word,
    },
};
use pumpkin_plugin_api::{
    Context,
    common::GameMode,
    permission::PermissionLevel,
};

pub fn register(ctx: &Context) {
    let op = Access::Op(PermissionLevel::Two);

    register_node(ctx, "gamemode.others", "Change another player's gamemode", op);
    register_node(ctx, "heal.others", "Heal another player", op);
    register_node(ctx, "feed.others", "Feed another player", op);
    register_node(ctx, "fly.others", "Toggle flight for another player", op);
    register_node(ctx, "god.others", "Toggle god mode for another player", op);
    register_node(ctx, "chat.color", "Use & colour codes in chat", op);

    // /gmc /gms /gma /gmsp (all take an optional player)
    command(ctx, &["gmc"], "Switch to creative mode", "gmc", op, |c| {
        c.then(player_arg("player").execute(run(|cx| set_mode(cx, GameMode::Creative, "Creative"))))
            .execute(run(|cx| set_mode(cx, GameMode::Creative, "Creative")))
    });
    command(ctx, &["gms"], "Switch to survival mode", "gms", op, |c| {
        c.then(player_arg("player").execute(run(|cx| set_mode(cx, GameMode::Survival, "Survival"))))
            .execute(run(|cx| set_mode(cx, GameMode::Survival, "Survival")))
    });
    command(ctx, &["gma"], "Switch to adventure mode", "gma", op, |c| {
        c.then(player_arg("player").execute(run(|cx| set_mode(cx, GameMode::Adventure, "Adventure"))))
            .execute(run(|cx| set_mode(cx, GameMode::Adventure, "Adventure")))
    });
    command(ctx, &["gmsp"], "Switch to spectator mode", "gmsp", op, |c| {
        c.then(player_arg("player").execute(run(|cx| set_mode(cx, GameMode::Spectator, "Spectator"))))
            .execute(run(|cx| set_mode(cx, GameMode::Spectator, "Spectator")))
    });
    command(ctx, &["gm"], "Switch gamemode: /gm <c|s|a|sp> [player]", "gm", op, |c| {
        c.then(
            word("mode")
                .then(player_arg("player").execute(run(gm_generic)))
                .execute(run(gm_generic)),
        )
    });

    command(ctx, &["fly"], "Toggle flight", "fly", op, |c| {
        c.then(player_arg("player").execute(run(fly))).execute(run(fly))
    });
    command(ctx, &["speed"], "Set fly/walk speed (1-10)", "speed", op, |c| {
        c.then(word("level").execute(run(speed)))
    });
    command(ctx, &["heal"], "Restore health and hunger", "heal", op, |c| {
        c.then(player_arg("player").execute(run(heal))).execute(run(heal))
    });
    command(ctx, &["feed"], "Restore hunger", "feed", op, |c| {
        c.then(player_arg("player").execute(run(feed))).execute(run(feed))
    });
    command(ctx, &["god"], "Toggle invulnerability", "god", op, |c| {
        c.then(player_arg("player").execute(run(god))).execute(run(god))
    });
    command(ctx, &["top"], "Teleport to the highest block above you", "top", Access::Everyone, |c| {
        c.execute(run(top))
    });
    command(ctx, &["ping"], "Show your latency", "ping", Access::Everyone, |c| {
        c.then(player_arg("player").execute(run(ping))).execute(run(ping))
    });
    command(ctx, &["broadcast", "bc"], "Broadcast a message to everyone", "broadcast", op, |c| {
        c.then(greedy("message").execute(run(broadcast)))
    });
    command(ctx, &["msg", "tell", "w", "m"], "Send a private message", "msg", Access::Everyone, |c| {
        c.then(player_arg("player").then(greedy("message").execute(run(msg))))
    });
    command(ctx, &["reply", "r"], "Reply to your last private message", "msg", Access::Everyone, |c| {
        c.then(greedy("message").execute(run(reply)))
    });
    command(ctx, &["seen"], "When did a player last play?", "seen", Access::Everyone, |c| {
        c.then(player_arg("player").execute(run(seen)))
    });
    command(ctx, &["suicide"], "Take your own life (dramatic)", "suicide", Access::Everyone, |c| {
        c.execute(run(suicide))
    });
}

// ---------------------------------------------------------------------------------------------

fn set_mode(cx: &Cx, mode: GameMode, label: &str) -> CmdResult {
    let target = cx.target_or_self("player")?;
    let me = cx.sender.as_player().map(|p| p.get_id().to_string());
    let other = me.as_deref() != Some(target.get_id().to_string().as_str());
    if other && !cx.has_perm("gamemode.others") {
        return Err(fail("You can't change other players' gamemode."));
    }
    target.set_gamemode(mode);
    tell(&target, ok(&format!("Your gamemode is now &e{label}&a.")));
    if other {
        cx.ok(&format!("Set &e{}&a's gamemode to &e{label}&a.", target.get_name()))
    } else {
        Ok(1)
    }
}

fn gm_generic(cx: &Cx) -> CmdResult {
    let raw = cx.require("mode", "/gm <c|s|a|sp> [player]")?.to_lowercase();
    match raw.as_str() {
        "c" | "1" | "creative" => set_mode(cx, GameMode::Creative, "Creative"),
        "s" | "0" | "survival" => set_mode(cx, GameMode::Survival, "Survival"),
        "a" | "2" | "adventure" => set_mode(cx, GameMode::Adventure, "Adventure"),
        "sp" | "3" | "spectator" => set_mode(cx, GameMode::Spectator, "Spectator"),
        _ => Err(fail("Unknown mode. Use c, s, a or sp.")),
    }
}

fn gate_others(cx: &Cx, target_uuid: &str, node: &str) -> Result<(), pumpkin_plugin_api::command::CommandError> {
    let me = cx.sender.as_player().map(|p| uuid_of(&p));
    if me.as_deref() != Some(target_uuid) && !cx.has_perm(node) {
        return Err(fail("You can't do that to other players."));
    }
    Ok(())
}

fn fly(cx: &Cx) -> CmdResult {
    let target = cx.target_or_self("player")?;
    gate_others(cx, &uuid_of(&target), "fly.others")?;
    let enable = !target.get_abilities().allow_flying;
    target.set_allow_flight(enable);
    if !enable {
        target.set_flying(false);
    }
    let state = if enable { "&aenabled" } else { "&cdisabled" };
    tell(&target, info(&format!("Flight {state}&7.")));
    if target.get_name() != cx.sender.get_name() {
        cx.info(&format!("Flight {state} &7for &f{}&7.", target.get_name()))?;
    }
    Ok(1)
}

fn speed(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let raw = cx.require("level", "/speed <1-10>")?;
    let level: f32 = raw.parse().map_err(|_| fail("Speed must be a number from 1 to 10."))?;
    if !(1.0..=10.0).contains(&level) {
        return Err(fail("Speed must be between 1 and 10."));
    }
    // Vanilla defaults are 0.05 flying and 0.1 walking, which we treat as level 1.
    if player.is_flying() {
        player.set_fly_speed(0.05 * level);
        tell(&player, ok(&format!("Fly speed set to &e{level}&a.")));
    } else {
        player.set_walk_speed(0.1 * level);
        tell(&player, ok(&format!("Walk speed set to &e{level}&a.")));
    }
    Ok(1)
}

fn heal(cx: &Cx) -> CmdResult {
    let target = cx.target_or_self("player")?;
    gate_others(cx, &uuid_of(&target), "heal.others")?;
    target.set_health(target.get_max_health());
    target.set_food_level(20);
    target.set_saturation(20.0);
    target.clear_effects();
    tell(&target, ok("You have been healed."));
    if target.get_name() != cx.sender.get_name() {
        cx.ok(&format!("Healed &e{}&a.", target.get_name()))?;
    }
    Ok(1)
}

fn feed(cx: &Cx) -> CmdResult {
    let target = cx.target_or_self("player")?;
    gate_others(cx, &uuid_of(&target), "feed.others")?;
    target.set_food_level(20);
    target.set_saturation(20.0);
    tell(&target, ok("Your hunger has been satisfied."));
    if target.get_name() != cx.sender.get_name() {
        cx.ok(&format!("Fed &e{}&a.", target.get_name()))?;
    }
    Ok(1)
}

fn god(cx: &Cx) -> CmdResult {
    let target = cx.target_or_self("player")?;
    let id = uuid_of(&target);
    gate_others(cx, &id, "god.others")?;
    let enable = rt(|r| {
        if r.god.remove(&id) {
            false
        } else {
            r.god.insert(id.clone());
            true
        }
    });
    target.set_invulnerable(enable);
    let state = if enable { "&aenabled" } else { "&cdisabled" };
    tell(&target, info(&format!("God mode {state}&7.")));
    if target.get_name() != cx.sender.get_name() {
        cx.info(&format!("God mode {state} &7for &f{}&7.", target.get_name()))?;
    }
    Ok(1)
}

fn top(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    let (x, _, z) = player.get_position();
    let world = player.get_world();
    let (bx, bz) = (x.floor() as i32, z.floor() as i32);
    let y = world.get_top_block_y(bx, bz);
    tp::remember_back(&player);
    player.teleport((x, f64::from(y) + 1.0, z), None, None, world);
    cx.ok("Whoosh! You're on top.")
}

fn ping(cx: &Cx) -> CmdResult {
    let target = cx.target_or_self("player")?;
    let ms = target.get_ping();
    let color = match ms {
        0..=80 => "&a",
        81..=200 => "&e",
        _ => "&c",
    };
    cx.info(&format!("&f{}&7's ping: {color}{ms}ms", target.get_name()))
}

fn broadcast(cx: &Cx) -> CmdResult {
    let message = cx.require("message", "/broadcast <message>")?;
    let line = format!("&6&l[Broadcast] &r&e{message}");
    for p in cx.server.get_all_players() {
        p.send_system_message(colored(&line), false);
    }
    tracing::info!("[Broadcast] {}", strip_codes(&message));
    Ok(1)
}

fn deliver_msg(cx: &Cx, target: &pumpkin_plugin_api::Player, message: &str) -> CmdResult {
    let from = cx.sender.get_name();
    let message = if cx.has_perm("chat.color") { message.to_string() } else { strip_codes(message) };
    target.send_system_message(colored(&format!("&8[&d{from} &7→ &dyou&8] &f{message}")), false);
    cx.reply(colored(&format!("&8[&dyou &7→ &d{}&8] &f{message}", target.get_name())));
    if let Some(me) = cx.sender.as_player() {
        let (a, b) = (uuid_of(&me), uuid_of(target));
        rt(|r| {
            r.reply.insert(b.clone(), a.clone());
            r.reply.insert(a, b);
        });
    }
    Ok(1)
}

fn msg(cx: &Cx) -> CmdResult {
    let name = cx.require("player", "/msg <player> <message>")?;
    let message = cx.require("message", "/msg <player> <message>")?;
    let target = cx.online(&name)?;
    deliver_msg(cx, &target, &message)
}

fn reply(cx: &Cx) -> CmdResult {
    let me = cx.player()?;
    let message = cx.require("message", "/r <message>")?;
    let last = rt(|r| r.reply.get(&uuid_of(&me)).cloned()).ok_or_else(|| fail("Nobody to reply to."))?;
    let target = cx
        .server
        .get_all_players()
        .into_iter()
        .find(|p| uuid_of(p) == last)
        .ok_or_else(|| fail("That player is no longer online."))?;
    deliver_msg(cx, &target, &message)
}

fn seen(cx: &Cx) -> CmdResult {
    let name = cx.require("player", "/seen <player>")?;
    if let Some(p) = crate::util::find_online(&cx.server, &name) {
        return cx.info(&format!("&f{} &7is &aonline &7right now.", p.get_name()));
    }
    let found = data::with(|d| d.find_user(&name).map(|(_, u)| (u.name.clone(), u.last_seen)));
    match found {
        Some((n, last)) if last > 0 => cx.info(&format!(
            "&f{n} &7was last seen &e{} &7ago.",
            fmt_duration(now_secs().saturating_sub(last))
        )),
        _ => Err(fail(&format!("I've never seen '{name}'."))),
    }
}

fn suicide(cx: &Cx) -> CmdResult {
    let player = cx.player()?;
    player.set_health(0.0);
    cx.reply(bad("You are no more."));
    Ok(1)
}
