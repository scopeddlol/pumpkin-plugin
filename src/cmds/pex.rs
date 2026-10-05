//! `/pex` — manage groups, users and permissions (PermissionsEx style).

use crate::{
    data,
    perms::{self, Group},
    tp::uuid_of,
    util::{
        Access, CmdResult, Cx, colored, command, fail, find_online, fmt_duration, greedy, lit, now_secs, strip_codes,
        suggestions, word,
    },
    util::run,
};
use pumpkin_plugin_api::{
    Context, Server,
    command::{CommandSender, CommandSuggestions, SuggestionRequest},
    commands::CommandSuggestionHandler,
    permission::PermissionLevel,
};

struct GroupNames;
impl CommandSuggestionHandler for GroupNames {
    fn suggest(&self, _: CommandSender, _: Server, req: SuggestionRequest) -> CommandSuggestions {
        let prefix = req.remaining.to_lowercase();
        let names = perms::read(|s| s.groups.keys().cloned().collect::<Vec<_>>());
        suggestions(&req, names.into_iter().filter(|n| n.starts_with(&prefix)).collect())
    }
}

fn group_arg(name: &str) -> pumpkin_plugin_api::command::CommandNode {
    word(name).suggest(GroupNames)
}

pub fn register(ctx: &Context) {
    command(ctx, &["pex", "permissions", "perms"], "Manage groups, users and permissions", "pex", Access::Op(PermissionLevel::Four), |c| {
        c.then(lit("reload").execute(run(reload)))
            .then(lit("groups").execute(run(groups)))
            .then(lit("check").then(word("player").then(word("node").execute(run(check)))))
            .then(
                lit("group").then(
                    group_arg("name")
                        .then(lit("info").execute(run(group_info)))
                        .then(lit("create").execute(run(group_create)))
                        .then(lit("delete").execute(run(group_delete)))
                        .then(lit("default").execute(run(group_default)))
                        .then(
                            lit("perm")
                                .then(lit("add").then(word("node").execute(run(group_perm_add))))
                                .then(lit("remove").then(word("node").execute(run(group_perm_remove)))),
                        )
                        .then(lit("prefix").then(greedy("value").execute(run(group_prefix))))
                        .then(lit("suffix").then(greedy("value").execute(run(group_suffix))))
                        .then(lit("weight").then(word("value").execute(run(group_weight))))
                        .then(lit("homes").then(word("value").execute(run(group_homes))))
                        .then(
                            lit("inherit")
                                .then(lit("add").then(group_arg("other").execute(run(inherit_add))))
                                .then(lit("remove").then(group_arg("other").execute(run(inherit_remove)))),
                        ),
                ),
            )
            .then(
                lit("user").then(
                    word("name")
                        .then(lit("info").execute(run(user_info)))
                        .then(
                            lit("group")
                                .then(
                                    lit("add").then(
                                        group_arg("group")
                                            .then(word("duration").execute(run(user_group_add)))
                                            .execute(run(user_group_add)),
                                    ),
                                )
                                .then(lit("remove").then(group_arg("group").execute(run(user_group_remove))))
                                .then(lit("set").then(group_arg("group").execute(run(user_group_set)))),
                        )
                        .then(
                            lit("perm")
                                .then(lit("add").then(word("node").execute(run(user_perm_add))))
                                .then(lit("remove").then(word("node").execute(run(user_perm_remove)))),
                        )
                        .then(lit("prefix").then(greedy("value").execute(run(user_prefix))))
                        .then(lit("suffix").then(greedy("value").execute(run(user_suffix)))),
                ),
            )
            .execute(run(help))
    });
}

// ---- helpers ----------------------------------------------------------------------------------

/// Success reply that also refreshes tab-list names, since prefixes may have changed.
fn done(cx: &Cx, text: &str) -> CmdResult {
    crate::events::refresh_tabs(&cx.server);
    cx.ok(text)
}

fn help(cx: &Cx) -> CmdResult {
    cx.reply(colored("&8&m      &r &6&lPermissions &8&m      "));
    for line in [
        "/pex groups &8- &7list groups",
        "/pex group <g> info|create|delete|default",
        "/pex group <g> perm add|remove <node>   &8(prefix with - to deny)",
        "/pex group <g> prefix|suffix <text>  &8| &7weight <n> &8| &7homes <n>",
        "/pex group <g> inherit add|remove <other>",
        "/pex user <p> info",
        "/pex user <p> group add <g> [30d] &8| &7remove <g> &8| &7set <g>",
        "/pex user <p> perm add|remove <node>",
        "/pex user <p> prefix|suffix <text|reset>",
        "/pex check <player> <node> &8- &7debug a permission check",
        "/pex reload",
    ] {
        cx.reply(colored(&format!(" &e{line}")));
    }
    Ok(1)
}

fn reload(cx: &Cx) -> CmdResult {
    perms::load();
    done(cx, "Permissions reloaded from disk.")
}

fn group_name(cx: &Cx) -> Result<String, pumpkin_plugin_api::command::CommandError> {
    Ok(cx.require("name", "/pex group <name> ...")?.to_lowercase())
}

fn existing_group(cx: &Cx) -> Result<String, pumpkin_plugin_api::command::CommandError> {
    let name = group_name(cx)?;
    if perms::read(|s| s.groups.contains_key(&name)) {
        Ok(name)
    } else {
        Err(fail(&format!("There is no group called '{name}'.")))
    }
}

/// Make sure prefixes end in a separator so `{prefix}{name}` never glues together.
fn prefix_fix(v: &str) -> String {
    let t = v.trim_end_matches(' ');
    let ends_with_code = t.len() >= 2 && t.as_bytes()[t.len() - 2] == b'&';
    if v.ends_with(' ') || ends_with_code { v.to_string() } else { format!("{t} ") }
}

fn suffix_fix(v: &str) -> String {
    if v.starts_with(' ') { v.to_string() } else { format!(" {v}") }
}

fn is_clear(v: &str) -> bool {
    matches!(v.to_lowercase().as_str(), "reset" | "none" | "clear" | "-")
}

fn clean_node(raw: &str) -> String {
    let (neg, body) = raw.strip_prefix('-').map_or(("", raw), |b| ("-", b));
    format!("{neg}{}", perms::normalize(body))
}

fn describe(entries: &[String]) -> String {
    if entries.is_empty() {
        return "&8none".into();
    }
    entries
        .iter()
        .map(|e| if e.starts_with('-') { format!("&c{e}") } else { format!("&a{e}") })
        .collect::<Vec<_>>()
        .join("&8, ")
}

// ---- groups -----------------------------------------------------------------------------------

fn groups(cx: &Cx) -> CmdResult {
    let (default, rows) = perms::read(|s| {
        let mut rows: Vec<(String, Group)> = s.groups.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        rows.sort_by_key(|(_, g)| std::cmp::Reverse(g.weight));
        (s.default_group.clone(), rows)
    });
    cx.reply(colored("&8&m      &r &6&lGroups &8&m      "));
    for (name, g) in rows {
        let tag = if name == default { " &8(default)" } else { "" };
        cx.reply(colored(&format!(
            " &e{name}{tag} &8- &7weight &f{} &8- &7{}&fPlayer",
            g.weight, g.prefix
        )));
    }
    Ok(1)
}

fn group_info(cx: &Cx) -> CmdResult {
    let name = existing_group(cx)?;
    let g = perms::read(|s| s.groups.get(&name).cloned()).unwrap_or_default();
    cx.reply(colored(&format!("&8&m      &r &6&lGroup {name} &8&m      ")));
    cx.reply(colored(&format!(" &7Weight: &f{}  &7Homes: &f{}", g.weight, g.homes.map_or("default".into(), |h| h.to_string()))));
    cx.reply(colored(&format!(" &7Prefix: &r{}&fPlayer{}", g.prefix, g.suffix)));
    cx.reply(colored(&format!(" &7Inherits: &f{}", if g.inherits.is_empty() { "none".into() } else { g.inherits.join(", ") })));
    cx.reply(colored(&format!(" &7Permissions: {}", describe(&g.permissions))));
    Ok(1)
}

fn group_create(cx: &Cx) -> CmdResult {
    let name = group_name(cx)?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(fail("Group names may only use letters, numbers, - and _."));
    }
    let created = perms::mutate(|s| {
        if s.groups.contains_key(&name) {
            false
        } else {
            s.groups.insert(name.clone(), Group::default());
            true
        }
    });
    if created { done(cx, &format!("Created group &e{name}&a.")) } else { Err(fail("That group already exists.")) }
}

fn group_delete(cx: &Cx) -> CmdResult {
    let name = existing_group(cx)?;
    if perms::read(|s| s.default_group == name) {
        return Err(fail("You can't delete the default group. Pick another default first."));
    }
    perms::mutate(|s| {
        s.groups.remove(&name);
        for g in s.groups.values_mut() {
            g.inherits.retain(|i| *i != name);
        }
        for u in s.users.values_mut() {
            u.groups.retain(|g| *g != name);
            u.expiries.remove(&name);
        }
    });
    done(cx, &format!("Deleted group &e{name}&a."))
}

fn group_default(cx: &Cx) -> CmdResult {
    let name = existing_group(cx)?;
    perms::mutate(|s| s.default_group = name.clone());
    done(cx, &format!("&e{name}&a is now the default group."))
}

fn group_perm_add(cx: &Cx) -> CmdResult {
    let name = existing_group(cx)?;
    let node = clean_node(&cx.require("node", "/pex group <g> perm add <node>")?);
    perms::mutate(|s| {
        if let Some(g) = s.groups.get_mut(&name) {
            // Granting and denying the same node are mutually exclusive.
            let bare = node.trim_start_matches('-').to_string();
            g.permissions.retain(|p| p.trim_start_matches('-') != bare);
            g.permissions.push(node.clone());
        }
    });
    done(cx, &format!("Group &e{name}&a: set &f{node}&a."))
}

fn group_perm_remove(cx: &Cx) -> CmdResult {
    let name = existing_group(cx)?;
    let bare = clean_node(&cx.require("node", "/pex group <g> perm remove <node>")?);
    let bare = bare.trim_start_matches('-').to_string();
    let removed = perms::mutate(|s| {
        s.groups.get_mut(&name).is_some_and(|g| {
            let before = g.permissions.len();
            g.permissions.retain(|p| p.trim_start_matches('-') != bare);
            before != g.permissions.len()
        })
    });
    if removed { done(cx, &format!("Group &e{name}&a: removed &f{bare}&a.")) } else { Err(fail("The group doesn't have that node.")) }
}

fn group_prefix(cx: &Cx) -> CmdResult {
    let name = existing_group(cx)?;
    let raw = cx.require("value", "/pex group <g> prefix <text|reset>")?;
    let value = if is_clear(&raw) { String::new() } else { prefix_fix(&raw) };
    perms::mutate(|s| {
        if let Some(g) = s.groups.get_mut(&name) {
            g.prefix = value.clone();
        }
    });
    done(cx, &format!("Group &e{name}&a prefix: &r{value}&fPlayer"))
}

fn group_suffix(cx: &Cx) -> CmdResult {
    let name = existing_group(cx)?;
    let raw = cx.require("value", "/pex group <g> suffix <text|reset>")?;
    let value = if is_clear(&raw) { String::new() } else { suffix_fix(&raw) };
    perms::mutate(|s| {
        if let Some(g) = s.groups.get_mut(&name) {
            g.suffix = value.clone();
        }
    });
    done(cx, &format!("Group &e{name}&a suffix: &fPlayer&r{value}"))
}

fn group_weight(cx: &Cx) -> CmdResult {
    let name = existing_group(cx)?;
    let weight: i32 = cx.require("value", "/pex group <g> weight <number>")?.parse().map_err(|_| fail("Weight must be a whole number."))?;
    perms::mutate(|s| {
        if let Some(g) = s.groups.get_mut(&name) {
            g.weight = weight;
        }
    });
    done(cx, &format!("Group &e{name}&a weight set to &f{weight}&a."))
}

fn group_homes(cx: &Cx) -> CmdResult {
    let name = existing_group(cx)?;
    let raw = cx.require("value", "/pex group <g> homes <number|reset>")?;
    let value = if is_clear(&raw) {
        None
    } else {
        Some(raw.parse::<u32>().map_err(|_| fail("Enter a whole number, or 'reset'."))?)
    };
    perms::mutate(|s| {
        if let Some(g) = s.groups.get_mut(&name) {
            g.homes = value;
        }
    });
    done(cx, &format!("Group &e{name}&a home limit: &f{}&a.", value.map_or("default".into(), |v| v.to_string())))
}

fn inherit_add(cx: &Cx) -> CmdResult {
    let name = existing_group(cx)?;
    let other = cx.require("other", "/pex group <g> inherit add <other>")?.to_lowercase();
    if !perms::read(|s| s.groups.contains_key(&other)) {
        return Err(fail(&format!("There is no group called '{other}'.")));
    }
    if other == name {
        return Err(fail("A group can't inherit from itself."));
    }
    perms::mutate(|s| {
        if let Some(g) = s.groups.get_mut(&name) && !g.inherits.contains(&other) {
            g.inherits.push(other.clone());
        }
    });
    done(cx, &format!("Group &e{name}&a now inherits &e{other}&a."))
}

fn inherit_remove(cx: &Cx) -> CmdResult {
    let name = existing_group(cx)?;
    let other = cx.require("other", "/pex group <g> inherit remove <other>")?.to_lowercase();
    perms::mutate(|s| {
        if let Some(g) = s.groups.get_mut(&name) {
            g.inherits.retain(|i| *i != other);
        }
    });
    done(cx, &format!("Group &e{name}&a no longer inherits &e{other}&a."))
}

// ---- users ------------------------------------------------------------------------------------

/// Resolve a user by name, creating a permission record for players that are online.
fn user_id(cx: &Cx) -> Result<(String, String), pumpkin_plugin_api::command::CommandError> {
    let name = cx.require("name", "/pex user <player> ...")?;
    if let Some(p) = find_online(&cx.server, &name) {
        let id = uuid_of(&p);
        perms::ensure_user(&id, &p.get_name());
        return Ok((id, p.get_name()));
    }
    perms::find_uuid(&name)
        .map(|id| (id, name.clone()))
        .or_else(|| data::with(|d| d.find_user(&name).map(|(id, u)| (id.clone(), u.name.clone()))))
        .map(|(id, n)| {
            perms::ensure_user(&id, &n);
            (id, n)
        })
        .ok_or_else(|| fail(&format!("I don't know a player called '{name}' (they must have joined at least once).")))
}

fn user_info(cx: &Cx) -> CmdResult {
    let (id, name) = user_id(cx)?;
    let u = perms::read(|s| s.users.get(&id).cloned()).unwrap_or_default();
    let now = now_secs();
    let groups: Vec<String> = perms::group_names(&id)
        .into_iter()
        .map(|g| match u.expiries.get(&g) {
            Some(exp) if *exp > now => format!("{g} &8({} left)&f", fmt_duration(exp - now)),
            _ => g,
        })
        .collect();
    cx.reply(colored(&format!("&8&m      &r &6&l{name} &8&m      ")));
    cx.reply(colored(&format!(" &7Groups: &f{}", groups.join(", "))));
    cx.reply(colored(&format!(" &7Display: &r{}{name}{}", perms::prefix(&id), perms::suffix(&id))));
    cx.reply(colored(&format!(" &7Own permissions: {}", describe(&u.permissions))));
    Ok(1)
}

fn require_group(cx: &Cx) -> Result<String, pumpkin_plugin_api::command::CommandError> {
    let g = cx.require("group", "/pex user <p> group <add|remove|set> <group>")?.to_lowercase();
    if perms::read(|s| s.groups.contains_key(&g)) { Ok(g) } else { Err(fail(&format!("There is no group called '{g}'."))) }
}

fn user_group_add(cx: &Cx) -> CmdResult {
    let (id, name) = user_id(cx)?;
    let group = require_group(cx)?;
    let expiry = match cx.arg("duration") {
        Some(d) => Some(now_secs() + perms::parse_duration(&d).ok_or_else(|| fail("Use a duration like 30m, 12h, 7d or 2w."))?),
        None => None,
    };
    perms::mutate(|s| {
        let u = s.users.entry(id.clone()).or_default();
        if !u.groups.contains(&group) {
            u.groups.push(group.clone());
        }
        match expiry {
            Some(e) => u.expiries.insert(group.clone(), e),
            None => u.expiries.remove(&group),
        };
    });
    let tail = expiry.map_or(String::new(), |e| format!(" &7for &f{}", fmt_duration(e - now_secs())));
    done(cx, &format!("Added &e{name}&a to group &e{group}&a{tail}&a."))
}

fn user_group_remove(cx: &Cx) -> CmdResult {
    let (id, name) = user_id(cx)?;
    let group = require_group(cx)?;
    perms::mutate(|s| {
        if let Some(u) = s.users.get_mut(&id) {
            u.groups.retain(|g| *g != group);
            u.expiries.remove(&group);
        }
    });
    done(cx, &format!("Removed &e{name}&a from group &e{group}&a."))
}

fn user_group_set(cx: &Cx) -> CmdResult {
    let (id, name) = user_id(cx)?;
    let group = require_group(cx)?;
    perms::mutate(|s| {
        let u = s.users.entry(id.clone()).or_default();
        u.groups = vec![group.clone()];
        u.expiries.clear();
    });
    done(cx, &format!("&e{name}&a is now only in group &e{group}&a."))
}

fn user_perm_add(cx: &Cx) -> CmdResult {
    let (id, name) = user_id(cx)?;
    let node = clean_node(&cx.require("node", "/pex user <p> perm add <node>")?);
    perms::mutate(|s| {
        let u = s.users.entry(id.clone()).or_default();
        let bare = node.trim_start_matches('-').to_string();
        u.permissions.retain(|p| p.trim_start_matches('-') != bare);
        u.permissions.push(node.clone());
    });
    done(cx, &format!("&e{name}&a: set &f{node}&a."))
}

fn user_perm_remove(cx: &Cx) -> CmdResult {
    let (id, name) = user_id(cx)?;
    let bare = clean_node(&cx.require("node", "/pex user <p> perm remove <node>")?);
    let bare = bare.trim_start_matches('-').to_string();
    let removed = perms::mutate(|s| {
        s.users.get_mut(&id).is_some_and(|u| {
            let before = u.permissions.len();
            u.permissions.retain(|p| p.trim_start_matches('-') != bare);
            before != u.permissions.len()
        })
    });
    if removed { done(cx, &format!("&e{name}&a: removed &f{bare}&a.")) } else { Err(fail("That player doesn't have that node set.")) }
}

fn user_prefix(cx: &Cx) -> CmdResult {
    let (id, name) = user_id(cx)?;
    let raw = cx.require("value", "/pex user <p> prefix <text|reset>")?;
    let value = (!is_clear(&raw)).then(|| prefix_fix(&raw));
    perms::mutate(|s| s.users.entry(id.clone()).or_default().prefix = value.clone());
    done(cx, &format!("&e{name}&a prefix: &r{}&f{name}", value.unwrap_or_default()))
}

fn user_suffix(cx: &Cx) -> CmdResult {
    let (id, name) = user_id(cx)?;
    let raw = cx.require("value", "/pex user <p> suffix <text|reset>")?;
    let value = (!is_clear(&raw)).then(|| suffix_fix(&raw));
    perms::mutate(|s| s.users.entry(id.clone()).or_default().suffix = value.clone());
    done(cx, &format!("&e{name}&a suffix: &f{name}&r{}", value.unwrap_or_default()))
}

fn check(cx: &Cx) -> CmdResult {
    let name = cx.require("player", "/pex check <player> <node>")?;
    let node = cx.require("node", "/pex check <player> <node>")?;
    let player = cx.online(&name)?;
    let ours = perms::check(&uuid_of(&player), &node);
    let effective = player.has_permission(&node);
    let verdict = match ours {
        Some(true) => "&agranted by PEX",
        Some(false) => "&cdenied by PEX",
        None => "&7no PEX entry (server default)",
    };
    cx.info(&format!(
        "&f{}&7 / &f{}&7: {verdict}&7 → effective: {}",
        player.get_name(),
        strip_codes(&node),
        if effective { "&aYES" } else { "&cNO" }
    ))?;
    Ok(1)
}
