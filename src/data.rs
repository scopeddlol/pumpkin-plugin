//! Persistent data (JSON files in the plugin data folder) and volatile runtime state.

use crate::util::Cents;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    sync::{LazyLock, Mutex, MutexGuard, OnceLock, RwLock},
};

static DIR: OnceLock<String> = OnceLock::new();

pub fn init_dir(dir: &str) {
    let _ = fs::create_dir_all(dir);
    let _ = DIR.set(dir.to_string());
}

fn path(name: &str) -> String {
    format!("{}/{name}", DIR.get().map_or(".", String::as_str))
}

/// Load `name` as JSON. A missing file yields (and writes) the default; a corrupt file is moved
/// aside to `<name>.bad` so we never silently overwrite someone's data.
pub fn load_json<T: Default + Serialize + DeserializeOwned>(name: &str) -> T {
    let p = path(name);
    match fs::read_to_string(&p) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            tracing::error!("{name} is invalid ({e}); keeping a copy as {name}.bad and starting fresh");
            let _ = fs::rename(&p, format!("{p}.bad"));
            T::default()
        }),
        Err(_) => {
            let value = T::default();
            save_json(name, &value);
            value
        }
    }
}

/// Write via a temp file + rename so a crash can't leave half a file.
pub fn save_json<T: Serialize>(name: &str, value: &T) {
    let p = path(name);
    let tmp = format!("{p}.tmp");
    match serde_json::to_string_pretty(value) {
        Ok(text) => {
            if fs::write(&tmp, text).and_then(|()| fs::rename(&tmp, &p)).is_err() {
                tracing::error!("failed to write {name}");
            }
        }
        Err(e) => tracing::error!("failed to serialise {name}: {e}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub currency_symbol: String,
    pub starting_balance: f64,
    pub default_homes: u32,
    pub tpa_timeout_secs: u64,
    pub teleport_warmup_secs: u64,
    pub teleport_cooldown_secs: u64,
    pub rtp_min_radius: i32,
    pub rtp_max_radius: i32,
    pub rtp_cooldown_secs: u64,
    pub rtp_max_attempts: u32,
    pub teleport_to_spawn_on_first_join: bool,
    pub market_tax_percent: f64,
    pub market_max_listings: usize,
    pub chat_format: String,
    pub chat_enabled: bool,
    pub join_message: String,
    pub leave_message: String,
    pub first_join_message: String,
    pub motd: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            currency_symbol: "$".into(),
            starting_balance: 100.0,
            default_homes: 3,
            tpa_timeout_secs: 60,
            teleport_warmup_secs: 3,
            teleport_cooldown_secs: 0,
            rtp_min_radius: 250,
            rtp_max_radius: 5000,
            rtp_cooldown_secs: 120,
            rtp_max_attempts: 25,
            teleport_to_spawn_on_first_join: true,
            market_tax_percent: 5.0,
            market_max_listings: 10,
            chat_format: "{prefix}{name}{suffix}&8 » &r{message}".into(),
            chat_enabled: true,
            join_message: "&8[&a+&8] &7{prefix}{name}".into(),
            leave_message: "&8[&c-&8] &7{prefix}{name}".into(),
            first_join_message: "&6✦ &eWelcome &f{name} &eto the server for the first time!".into(),
            motd: vec![
                "&6&m          &r &e&lWelcome back, {name}! &6&m          ".into(),
                "&7Balance: &a{balance} &8| &7Online: &f{online} &8| &7Rank: &f{group}".into(),
                "&7Type &e/help &7to see commands.".into(),
            ],
        }
    }
}

static CONFIG: LazyLock<RwLock<Config>> = LazyLock::new(|| RwLock::new(Config::default()));

pub fn load_config() {
    let cfg: Config = load_json("config.json");
    *CONFIG.write().unwrap_or_else(|e| e.into_inner()) = cfg;
}

pub fn config() -> Config {
    CONFIG.read().unwrap_or_else(|e| e.into_inner()).clone()
}

// ---------------------------------------------------------------------------------------------
// Persistent player/server data
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Loc {
    pub world: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct User {
    pub name: String,
    pub balance: Cents,
    pub homes: BTreeMap<String, Loc>,
    pub first_join: u64,
    pub last_seen: u64,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Data {
    /// Keyed by UUID string.
    pub users: BTreeMap<String, User>,
    pub warps: BTreeMap<String, Loc>,
    pub spawn: Option<Loc>,
}

impl Data {
    /// Case-insensitive lookup of a known (possibly offline) player by name.
    pub fn find_user(&self, name: &str) -> Option<(&String, &User)> {
        self.users
            .iter()
            .find(|(_, u)| u.name.eq_ignore_ascii_case(name))
    }
}

static DATA: LazyLock<Mutex<Data>> = LazyLock::new(|| Mutex::new(Data::default()));

pub fn load_data() {
    *lock(&DATA) = load_json("data.json");
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read access. Never call into the host while inside `f`.
pub fn with<R>(f: impl FnOnce(&Data) -> R) -> R {
    f(&lock(&DATA))
}

/// Write access; persists to disk afterwards.
pub fn mutate<R>(f: impl FnOnce(&mut Data) -> R) -> R {
    let mut guard = lock(&DATA);
    let result = f(&mut guard);
    save_json("data.json", &*guard);
    result
}

pub fn save_data() {
    save_json("data.json", &*lock(&DATA));
}

// ---------------------------------------------------------------------------------------------
// Volatile runtime state (lost on restart by design)
// ---------------------------------------------------------------------------------------------

pub struct TpaRequest {
    pub from: String,
    pub to: String,
    /// `true` = `from` teleports to `to` (/tpa), `false` = `to` teleports to `from` (/tpahere).
    pub from_goes_to_target: bool,
    pub expires: u64,
}

pub struct MarketView {
    pub page: usize,
    /// Listing ids shown in slots 0.., in order.
    pub ids: Vec<u64>,
    pub inv: pumpkin_plugin_api::Inventory,
}

#[derive(Default)]
pub struct Runtime {
    pub tpa: Vec<TpaRequest>,
    /// uuid -> last location before a teleport/death, for /back.
    pub back: HashMap<String, Loc>,
    pub god: HashSet<String>,
    /// uuid -> uuid of the last player who messaged them (for /r).
    pub reply: HashMap<String, String>,
    pub cooldowns: HashMap<(String, &'static str), u64>,
    /// uuid -> monotonically increasing token; a newer warmup invalidates an older one.
    pub warmups: HashMap<String, u64>,
    /// uuid -> the market window they have open.
    pub market_open: HashMap<String, MarketView>,
    pub next_token: u64,
}

static RUNTIME: LazyLock<Mutex<Runtime>> = LazyLock::new(|| Mutex::new(Runtime::default()));

pub fn rt<R>(f: impl FnOnce(&mut Runtime) -> R) -> R {
    f(&mut lock(&RUNTIME))
}
