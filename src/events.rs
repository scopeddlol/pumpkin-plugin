//! Event handlers: join/leave, chat formatting, permission checks, market clicks, death tracking.

use crate::{
    cmds::{economy, market},
    data::{self, config, mutate, rt},
    perms,
    tp::{self, uuid_of},
    util::{colored, now_secs, strip_codes, tell},
};
use pumpkin_plugin_api::{
    Context, Player, Server,
    events::{
        EventData, EventHandler, EventPriority, InventoryClickEvent, InventoryCloseEvent, PlayerChatEvent,
        PlayerDeathEvent, PlayerJoinEvent, PlayerLeaveEvent, PlayerPermissionCheckEvent,
    },
    permission::PermissionLevel,
};

pub fn register(ctx: &Context) -> Result<(), String> {
    let p = EventPriority::Normal;
    ctx.register_event_handler::<PlayerJoinEvent, _>(OnJoin, p, true)?;
    ctx.register_event_handler::<PlayerLeaveEvent, _>(OnLeave, p, true)?;
    ctx.register_event_handler::<PlayerChatEvent, _>(OnChat, p, true)?;
    ctx.register_event_handler::<PlayerPermissionCheckEvent, _>(OnPermCheck, p, true)?;
    ctx.register_event_handler::<PlayerDeathEvent, _>(OnDeath, p, true)?;
    ctx.register_event_handler::<InventoryClickEvent, _>(OnClick, p, true)?;
    ctx.register_event_handler::<InventoryCloseEvent, _>(OnClose, p, true)?;
    Ok(())
}

/// Replace `{placeholders}` for a player.
fn fill(template: &str, player: &Player, server: &Server) -> String {
    let id = uuid_of(player);
    template
        .replace("{name}", &player.get_name())
        .replace("{prefix}", &perms::prefix(&id))
        .replace("{suffix}", &perms::suffix(&id))
        .replace("{group}", &perms::primary_group(&id))
        .replace("{balance}", &economy::money(economy::balance(&id)))
        .replace("{online}", &server.get_player_count().to_string())
}

pub fn tab_name(player: &Player) -> String {
    let id = uuid_of(player);
    format!("{}{}{}", perms::prefix(&id), player.get_name(), perms::suffix(&id))
}

/// Re-apply prefixes/suffixes to every online player's tab list entry.
pub fn refresh_tabs(server: &Server) {
    for p in server.get_all_players() {
        p.set_tab_list_name(Some(colored(&tab_name(&p))));
    }
}

struct OnJoin;
impl EventHandler<PlayerJoinEvent> for OnJoin {
    fn handle(&self, server: Server, mut ev: EventData<PlayerJoinEvent>) -> EventData<PlayerJoinEvent> {
        let player = &ev.player;
        let id = uuid_of(player);
        let name = player.get_name();
        let cfg = config();

        let first_time = data::with(|d| d.users.get(&id).is_none_or(|u| u.first_join == 0));
        economy::ensure_account(&id, &name);
        let now = now_secs();
        mutate(|d| {
            let u = d.users.entry(id.clone()).or_default();
            if u.first_join == 0 {
                u.first_join = now;
            }
            u.last_seen = now;
        });
        perms::ensure_user(&id, &name);
        rt(|r| {
            r.god.remove(&id);
        });

        let msg = if first_time { &cfg.first_join_message } else { &cfg.join_message };
        ev.join_message = colored(&fill(msg, player, &server));
        player.set_tab_list_name(Some(colored(&tab_name(player))));

        for line in &cfg.motd {
            tell(player, colored(&fill(line, player, &server)));
        }
        if first_time && cfg.teleport_to_spawn_on_first_join {
            if let Some(spawn) = data::with(|d| d.spawn.clone()) {
                let _ = tp::teleport_now(&server, player, &spawn);
            }
        }
        ev
    }
}

struct OnLeave;
impl EventHandler<PlayerLeaveEvent> for OnLeave {
    fn handle(&self, server: Server, mut ev: EventData<PlayerLeaveEvent>) -> EventData<PlayerLeaveEvent> {
        let player = &ev.player;
        let id = uuid_of(player);
        let name = player.get_name();
        mutate(|d| d.users.entry(id.clone()).or_default().last_seen = now_secs());
        rt(|r| {
            r.tpa.retain(|t| t.from != name && t.to != name);
            r.market_open.remove(&id);
            r.warmups.remove(&id);
            r.god.remove(&id);
            r.reply.remove(&id);
        });
        ev.leave_message = colored(&fill(&config().leave_message, player, &server));
        ev
    }
}

struct OnChat;
impl EventHandler<PlayerChatEvent> for OnChat {
    fn handle(&self, server: Server, mut ev: EventData<PlayerChatEvent>) -> EventData<PlayerChatEvent> {
        let cfg = config();
        if !cfg.chat_enabled {
            return ev;
        }
        let player = &ev.player;
        let allow_color = player.has_permission(&format!("{}:chat.color", crate::util::NS));
        let message = if allow_color { ev.message.clone() } else { strip_codes(&ev.message) };

        let line = fill(&cfg.chat_format, player, &server).replace("{message}", &message);
        let component = colored(&line);

        // We deliver the formatted line ourselves, so the original must not also go out.
        ev.cancelled = true;
        let sender_name = player.get_name();
        let mut delivered_to_sender = false;
        for r in &ev.recipients {
            delivered_to_sender |= r.get_name() == sender_name;
            tell(r, colored(&line));
        }
        if !delivered_to_sender {
            tell(player, component);
        }
        tracing::info!("{}: {}", sender_name, strip_codes(&message));
        ev
    }
}

struct OnPermCheck;
impl EventHandler<PlayerPermissionCheckEvent> for OnPermCheck {
    fn handle(&self, _: Server, mut ev: EventData<PlayerPermissionCheckEvent>) -> EventData<PlayerPermissionCheckEvent> {
        let node = perms::normalize(&ev.permission);
        // Lock-out protection: a level-4 operator can always reach /pex.
        if node == "essentials.pex" && ev.player.get_permission_level() == PermissionLevel::Four {
            ev.permission_result = true;
            return ev;
        }
        if let Some(verdict) = perms::check(&uuid_of(&ev.player), &node) {
            ev.permission_result = verdict;
        }
        ev
    }
}

struct OnDeath;
impl EventHandler<PlayerDeathEvent> for OnDeath {
    fn handle(&self, _: Server, ev: EventData<PlayerDeathEvent>) -> EventData<PlayerDeathEvent> {
        tp::remember_back(&ev.player);
        ev
    }
}

struct OnClick;
impl EventHandler<InventoryClickEvent> for OnClick {
    fn handle(&self, server: Server, mut ev: EventData<InventoryClickEvent>) -> EventData<InventoryClickEvent> {
        if market::is_viewing(&ev.player) {
            // Nothing may move in or out of the market window.
            ev.cancelled = true;
            market::on_click(&server, &ev.player, ev.raw_slot);
        }
        ev
    }
}

struct OnClose;
impl EventHandler<InventoryCloseEvent> for OnClose {
    fn handle(&self, _: Server, ev: EventData<InventoryCloseEvent>) -> EventData<InventoryCloseEvent> {
        market::on_close(&ev.player);
        ev
    }
}
