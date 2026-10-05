//! Teleport helpers: locations, warm-ups, cooldowns, /back tracking and safe random spots.

use crate::{
    data::{self, Loc, config, rt},
    util::{self, fmt_duration, now_secs},
};
use pumpkin_plugin_api::{
    Player, Server, common::BlockPos, scheduler::SchedulerExt, world::World,
};

pub fn uuid_of(player: &Player) -> String {
    player.get_id().to_string()
}

pub fn loc_of(player: &Player) -> Loc {
    let (x, y, z) = player.get_position();
    Loc {
        world: player.get_world().get_name(),
        x,
        y,
        z,
        yaw: player.get_yaw(),
        pitch: player.get_pitch(),
    }
}

pub fn world_by_name(server: &Server, name: &str) -> Option<World> {
    server.get_world_by_name(name)
}

/// Pretty `world 12, 64, -30`.
pub fn fmt_loc(loc: &Loc) -> String {
    format!("{} {:.0}, {:.0}, {:.0}", loc.world, loc.x, loc.y, loc.z)
}

/// Remember where the player is so `/back` can return them.
pub fn remember_back(player: &Player) {
    let loc = loc_of(player);
    rt(|r| r.back.insert(uuid_of(player), loc));
}

/// Teleport right now, recording the previous spot for `/back`.
pub fn teleport_now(server: &Server, player: &Player, dest: &Loc) -> Result<(), String> {
    let world = world_by_name(server, &dest.world)
        .ok_or_else(|| format!("The world '{}' is not loaded.", dest.world))?;
    remember_back(player);
    player.teleport((dest.x, dest.y, dest.z), Some(dest.yaw), Some(dest.pitch), world);
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Cooldowns
// ---------------------------------------------------------------------------------------------

/// Seconds left on a named cooldown for this player (0 = ready).
pub fn cooldown_left(uuid: &str, key: &'static str) -> u64 {
    rt(|r| {
        r.cooldowns
            .get(&(uuid.to_string(), key))
            .map_or(0, |until| until.saturating_sub(now_secs()))
    })
}

pub fn start_cooldown(uuid: &str, key: &'static str, secs: u64) {
    if secs > 0 {
        rt(|r| r.cooldowns.insert((uuid.to_string(), key), now_secs() + secs));
    }
}

// ---------------------------------------------------------------------------------------------
// Warm-up teleport
// ---------------------------------------------------------------------------------------------

/// Teleport with the configured warm-up. Moving more than a block cancels it. Players with
/// `essentials.tp.bypass` go instantly and ignore the cooldown.
pub fn teleport_with_warmup(server: &Server, player: &Player, dest: Loc, label: &str) -> Result<(), String> {
    let cfg = config();
    let uuid = uuid_of(player);
    let bypass = player.has_permission(&format!("{}:tp.bypass", util::NS));

    let left = cooldown_left(&uuid, "teleport");
    if left > 0 && !bypass {
        return Err(format!("You can teleport again in {}.", fmt_duration(left)));
    }

    let warmup = if bypass { 0 } else { cfg.teleport_warmup_secs };
    if warmup == 0 {
        teleport_now(server, player, &dest)?;
        util::tell(player, util::ok(&format!("Teleported to &e{label}&a.")));
        start_cooldown(&uuid, "teleport", cfg.teleport_cooldown_secs);
        return Ok(());
    }

    let token = rt(|r| {
        r.next_token += 1;
        r.warmups.insert(uuid.clone(), r.next_token);
        r.next_token
    });
    let start = player.get_position();
    util::tell(
        player,
        util::info(&format!("Teleporting to &e{label} &7in &f{warmup}s&7 — &cdon't move&7!")),
    );

    let name = player.get_name();
    let label = label.to_string();
    let cooldown = cfg.teleport_cooldown_secs;
    server.schedule_delayed_task(warmup * 20, move |server| {
        // A newer teleport request replaces this one.
        if rt(|r| r.warmups.get(&uuid).copied()) != Some(token) {
            return;
        }
        rt(|r| r.warmups.remove(&uuid));
        let Some(player) = util::find_online(&server, &name) else {
            return;
        };
        let (x, y, z) = player.get_position();
        let moved = (x - start.0).powi(2) + (y - start.1).powi(2) + (z - start.2).powi(2);
        if moved > 1.0 {
            util::tell(&player, util::bad("Teleport cancelled because you moved."));
            return;
        }
        match teleport_now(&server, &player, &dest) {
            Ok(()) => {
                util::tell(&player, util::ok(&format!("Teleported to &e{label}&a.")));
                start_cooldown(&uuid, "teleport", cooldown);
            }
            Err(e) => util::tell(&player, util::bad(&e)),
        }
    });
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Random teleport
// ---------------------------------------------------------------------------------------------

/// xorshift64* seeded from the wall clock — plenty for picking a random spot.
pub struct Rng(u64);

impl Rng {
    pub fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9E37_79B9_7F4A_7C15, |d| d.as_nanos() as u64);
        Self(nanos | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform integer in `[-limit, limit]`.
    pub fn range(&mut self, limit: i32) -> i32 {
        let span = u64::from(limit.unsigned_abs()) * 2 + 1;
        (self.next() % span) as i32 - limit
    }
}

fn is_air(name: &str) -> bool {
    matches!(name, "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air")
}

const HAZARDS: &[&str] = &[
    "water", "lava", "fire", "cactus", "magma", "campfire", "powder_snow", "berry_bush",
    "dripstone", "leaves", "web", "wither_rose", "bubble_column", "kelp", "seagrass", "ice",
];

fn safe_floor(name: &str) -> bool {
    !is_air(name) && !HAZARDS.iter().any(|h| name.contains(h))
}

/// A safe standing spot at (x, z), if any, as the y of the player's feet.
fn safe_spot(world: &World, x: i32, z: i32) -> Option<i32> {
    let top = world.get_top_block_y(x, z);
    let block = |y: i32| world.get_block(BlockPos { x, y, z }).name;
    // Heightmap conventions differ by one; try both candidates for the feet position.
    [top + 1, top].into_iter().find(|&feet| {
        feet > world.get_min_y()
            && safe_floor(&block(feet - 1))
            && is_air(&block(feet))
            && is_air(&block(feet + 1))
    })
}

/// Search for a safe random location within the configured ring around the world spawn.
pub fn find_random_spot(server: &Server, player: &Player) -> Option<Loc> {
    let cfg = config();
    let world = player.get_world();
    if world.get_dimension() != "minecraft:overworld" {
        return None;
    }
    let spawn = world.get_spawn_location().pos;
    let border = world.get_world_border();
    let max = cfg.rtp_max_radius.max(cfg.rtp_min_radius + 1);
    let mut rng = Rng::new();
    let _ = server;

    for _ in 0..cfg.rtp_max_attempts {
        let dx = rng.range(max);
        let dz = rng.range(max);
        if dx.abs().max(dz.abs()) < cfg.rtp_min_radius {
            continue;
        }
        let (x, z) = (spawn.x + dx, spawn.z + dz);
        if !border.contains(f64::from(x), f64::from(z)) {
            continue;
        }
        if let Some(y) = safe_spot(&world, x, z) {
            return Some(Loc {
                world: world.get_name(),
                x: f64::from(x) + 0.5,
                y: f64::from(y),
                z: f64::from(z) + 0.5,
                yaw: player.get_yaw(),
                pitch: 0.0,
            });
        }
    }
    None
}

/// Drop expired /tpa requests.
pub fn prune_requests() {
    let now = now_secs();
    rt(|r| r.tpa.retain(|t| t.expires > now));
}

pub fn spawn_loc(server: &Server) -> Option<Loc> {
    if let Some(loc) = data::with(|d| d.spawn.clone()) {
        return Some(loc);
    }
    let worlds = server.get_all_worlds();
    let world = worlds
        .iter()
        .position(|w| w.get_dimension() == "minecraft:overworld")
        .map_or_else(|| worlds.first(), |i| worlds.get(i))?;
    let s = world.get_spawn_location();
    Some(Loc {
        world: world.get_name(),
        x: f64::from(s.pos.x) + 0.5,
        y: f64::from(s.pos.y),
        z: f64::from(s.pos.z) + 0.5,
        yaw: s.yaw,
        pitch: s.pitch,
    })
}
