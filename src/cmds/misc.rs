//! Ender chest and the /essentials admin command.

use crate::{
    data,
    perms,
    util::{Access, CmdResult, Cx, colored, command, lit, run},
};
use pumpkin_plugin_api::{Context, command_wit::PermissionLevel as CommandLevel};

pub fn register(ctx: &Context) {
    command(ctx, &["enderchest", "ec", "echest"], "Open your ender chest anywhere", "enderchest", Access::Everyone, |c| {
        c.execute(run(enderchest))
    });
    command(ctx, &["essentials", "ess"], "Essentials info and administration", "essentials", Access::Everyone, |c| {
        c.then(lit("reload").execute(run(reload)))
            .then(lit("help").execute(run(help)))
            .execute(run(help))
    });
}

fn enderchest(cx: &Cx) -> CmdResult {
    cx.player()?.open_ender_chest();
    Ok(1)
}

fn reload(cx: &Cx) -> CmdResult {
    if !cx.sender.has_permission_level(CommandLevel::Three) && !cx.has_perm("reload") {
        return Err(crate::util::fail("You don't have permission to reload."));
    }
    data::load_config();
    data::load_data();
    perms::load();
    crate::cmds::market::load();
    crate::events::refresh_tabs(&cx.server);
    cx.ok("Reloaded config, data, market and permissions.")
}

fn help(cx: &Cx) -> CmdResult {
    cx.reply(colored("&8&m        &r &6&lEssentials &8&m        "));
    let sections: [(&str, &[&str]); 8] = [
        ("Gamemode", &["/gmc /gms /gma /gmsp", "/gm <mode> [player]"]),
        ("Player", &["/fly /speed /heal /feed /god", "/ping /top /seen /suicide"]),
        ("Travel", &["/home /sethome /delhome /homes", "/warp /warps /setwarp /delwarp", "/spawn /setspawn /back /rtp"]),
        ("Requests", &["/tpa /tpahere /tpaccept /tpdeny /tpcancel"]),
        ("Money", &["/bal /pay /baltop", "/eco give|take|set|reset"]),
        ("Market", &["/market, /market sell <price>", "/market mine, /market cancel <id>"]),
        ("Chat", &["/msg /reply /broadcast"]),
        ("Misc", &["/enderchest, /pex, /essentials reload"]),
    ];
    for (title, lines) in sections {
        cx.reply(colored(&format!(" &6{title}")));
        for line in lines {
            cx.reply(colored(&format!("   &e{line}")));
        }
    }
    Ok(1)
}
