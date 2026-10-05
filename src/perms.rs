//! A small PermissionsEx-style engine: groups with inheritance, per-user overrides, wildcards,
//! negation (`-node`), prefixes/suffixes, weights and temporary ranks.
//!
//! Resolution order (first layer with a matching entry wins):
//!   1. permissions set directly on the user
//!   2. the user's groups, highest `weight` first, each followed by the groups it inherits
//!
//! Inside one layer the most specific entry wins (`a.b.c` > `a.b.*` > `a.*` > `*`); on a tie a
//! negation (`-a.b`) beats a grant.

use crate::{data, util::now_secs};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    sync::{LazyLock, Mutex, MutexGuard},
};

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Group {
    pub permissions: Vec<String>,
    pub inherits: Vec<String>,
    pub prefix: String,
    pub suffix: String,
    /// Higher weight = more important when a player has several groups.
    pub weight: i32,
    /// Overrides the `/sethome` limit.
    pub homes: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct PUser {
    pub name: String,
    pub groups: Vec<String>,
    /// group -> unix timestamp after which the membership no longer applies.
    pub expiries: BTreeMap<String, u64>,
    pub permissions: Vec<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub default_group: String,
    pub groups: BTreeMap<String, Group>,
    pub users: BTreeMap<String, PUser>,
}

impl Default for Store {
    fn default() -> Self {
        let g = |perms: &[&str], inherits: &[&str], prefix: &str, weight: i32, homes: Option<u32>| Group {
            permissions: perms.iter().map(|s| (*s).to_string()).collect(),
            inherits: inherits.iter().map(|s| (*s).to_string()).collect(),
            prefix: prefix.to_string(),
            suffix: String::new(),
            weight,
            homes,
        };
        let mut groups = BTreeMap::new();
        groups.insert("default".into(), g(&[], &[], "&7", 0, None));
        groups.insert("vip".into(), g(&["essentials.chat.color"], &["default"], "&6[VIP] &e", 10, Some(5)));
        groups.insert(
            "moderator".into(),
            g(
                &["essentials.fly", "essentials.heal", "essentials.feed", "essentials.god", "essentials.back", "essentials.tp.bypass"],
                &["vip"],
                "&2[Mod] &a",
                50,
                Some(8),
            ),
        );
        groups.insert("admin".into(), g(&["*"], &["moderator"], "&c[Admin] &6", 100, Some(50)));
        Self {
            default_group: "default".into(),
            groups,
            users: BTreeMap::new(),
        }
    }
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

fn lock() -> MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn load() {
    let mut store: Store = data::load_json("permissions.json");
    // Guarantee the default group exists so every player resolves to something.
    store.groups.entry(store.default_group.clone()).or_default();
    *lock() = store;
}

pub fn read<R>(f: impl FnOnce(&Store) -> R) -> R {
    f(&lock())
}

pub fn mutate<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut guard = lock();
    let result = f(&mut guard);
    data::save_json("permissions.json", &*guard);
    result
}

/// `essentials:fly` and `Essentials.Fly` both become `essentials.fly`.
pub fn normalize(node: &str) -> String {
    node.trim().to_lowercase().replacen(':', ".", 1)
}

// ---------------------------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------------------------

/// Names of groups a user currently belongs to (expired ones dropped, default always present).
fn active_groups(store: &Store, user: Option<&PUser>) -> Vec<String> {
    let now = now_secs();
    let mut out: Vec<String> = user
        .map(|u| {
            u.groups
                .iter()
                .filter(|g| u.expiries.get(*g).is_none_or(|exp| *exp > now))
                .filter(|g| store.groups.contains_key(*g))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if !out.contains(&store.default_group) {
        out.push(store.default_group.clone());
    }
    out.sort_by_key(|g| std::cmp::Reverse(store.groups.get(g).map_or(0, |gr| gr.weight)));
    out
}

/// Groups ordered by precedence: each group followed by whatever it inherits (depth first).
fn expand<'a>(store: &'a Store, roots: &[String]) -> Vec<(&'a str, &'a Group)> {
    fn walk<'a>(store: &'a Store, name: &str, seen: &mut HashSet<String>, out: &mut Vec<(&'a str, &'a Group)>) {
        if !seen.insert(name.to_string()) {
            return;
        }
        if let Some((key, group)) = store.groups.get_key_value(name) {
            out.push((key.as_str(), group));
            let mut parents: Vec<&String> = group.inherits.iter().collect();
            parents.sort_by_key(|p| std::cmp::Reverse(store.groups.get(*p).map_or(0, |g| g.weight)));
            for parent in parents {
                walk(store, parent, seen, out);
            }
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for root in roots {
        walk(store, root, &mut seen, &mut out);
    }
    out
}

/// Specificity of `entry` for `node`, or `None` if it doesn't match. Higher is more specific.
fn match_entry(entry: &str, node: &str) -> Option<usize> {
    if entry == "*" {
        return Some(0);
    }
    if entry == node {
        return Some(usize::MAX);
    }
    let prefix = entry.strip_suffix(".*")?;
    (node == prefix || node.starts_with(&format!("{prefix}."))).then(|| prefix.len() + 1)
}

fn layer_verdict(entries: &[String], node: &str) -> Option<bool> {
    let mut best: Option<(usize, bool)> = None;
    for raw in entries {
        let (negated, entry) = raw.strip_prefix('-').map_or((false, raw.as_str()), |e| (true, e));
        let entry = normalize(entry);
        if let Some(spec) = match_entry(&entry, node) {
            let grant = !negated;
            best = match best {
                Some((s, _)) if s > spec => best,
                Some((s, g)) if s == spec => Some((s, g && grant)),
                _ => Some((spec, grant)),
            };
        }
    }
    best.map(|(_, grant)| grant)
}

/// Our verdict for `node`, or `None` when no configured entry says anything (so the server's
/// own default — e.g. "ops only" — still applies).
pub fn check(uuid: &str, node: &str) -> Option<bool> {
    check_in(&lock(), uuid, node)
}

fn check_in(store: &Store, uuid: &str, node: &str) -> Option<bool> {
    let node = normalize(node);
    let user = store.users.get(uuid);
    if let Some(user) = user
        && let Some(v) = layer_verdict(&user.permissions, &node)
    {
        return Some(v);
    }
    let roots = active_groups(store, user);
    expand(store, &roots)
        .into_iter()
        .find_map(|(_, g)| layer_verdict(&g.permissions, &node))
}

/// The user's highest-weight active group.
pub fn primary_group(uuid: &str) -> String {
    let store = lock();
    active_groups(&store, store.users.get(uuid))
        .into_iter()
        .next()
        .unwrap_or_else(|| store.default_group.clone())
}

fn first_group_value(uuid: &str, pick: impl Fn(&Group) -> &str) -> String {
    let store = lock();
    let roots = active_groups(&store, store.users.get(uuid));
    expand(&store, &roots)
        .into_iter()
        .map(|(_, g)| pick(g))
        .find(|v| !v.is_empty())
        .unwrap_or_default()
        .to_string()
}

pub fn prefix(uuid: &str) -> String {
    if let Some(p) = read(|s| s.users.get(uuid).and_then(|u| u.prefix.clone())) {
        return p;
    }
    first_group_value(uuid, |g| g.prefix.as_str())
}

pub fn suffix(uuid: &str) -> String {
    if let Some(p) = read(|s| s.users.get(uuid).and_then(|u| u.suffix.clone())) {
        return p;
    }
    first_group_value(uuid, |g| g.suffix.as_str())
}

pub fn homes_limit(uuid: &str, default: u32) -> u32 {
    let store = lock();
    let roots = active_groups(&store, store.users.get(uuid));
    expand(&store, &roots)
        .into_iter()
        .filter_map(|(_, g)| g.homes)
        .max()
        .unwrap_or(default)
}

pub fn ensure_user(uuid: &str, name: &str) {
    let needs = read(|s| s.users.get(uuid).is_none_or(|u| u.name != name));
    if needs {
        mutate(|s| s.users.entry(uuid.to_string()).or_default().name = name.to_string());
    }
}

/// Find a stored permission user by name (offline-friendly).
pub fn find_uuid(name: &str) -> Option<String> {
    read(|s| {
        s.users
            .iter()
            .find(|(_, u)| u.name.eq_ignore_ascii_case(name))
            .map(|(id, _)| id.clone())
    })
}

pub fn group_names(uuid: &str) -> Vec<String> {
    let store = lock();
    active_groups(&store, store.users.get(uuid))
}

/// Parses `30m`, `12h`, `7d`, `2w` (or bare seconds).
pub fn parse_duration(input: &str) -> Option<u64> {
    let s = input.trim().to_lowercase();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if split == 0 {
        return None;
    }
    let (num, unit) = s.split_at(split);
    let n: u64 = num.parse().ok()?;
    let mult = match unit {
        "s" | "" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        _ => return None,
    };
    n.checked_mul(mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_specificity() {
        let entries = vec!["essentials.*".to_string(), "-essentials.fly".to_string()];
        assert_eq!(layer_verdict(&entries, "essentials.fly"), Some(false));
        assert_eq!(layer_verdict(&entries, "essentials.home"), Some(true));
        assert_eq!(layer_verdict(&entries, "other.thing"), None);
    }

    #[test]
    fn star_matches_everything_but_loses_to_specific() {
        let entries = vec!["*".to_string(), "-minecraft.command.stop".to_string()];
        assert_eq!(layer_verdict(&entries, "anything.at.all"), Some(true));
        assert_eq!(layer_verdict(&entries, "minecraft.command.stop"), Some(false));
    }

    #[test]
    fn negation_wins_ties() {
        let entries = vec!["a.b".to_string(), "-a.b".to_string()];
        assert_eq!(layer_verdict(&entries, "a.b"), Some(false));
    }

    #[test]
    fn normalizes_namespaces() {
        assert_eq!(normalize("Essentials:Fly"), "essentials.fly");
        let entries = vec!["essentials:fly".to_string()];
        assert_eq!(layer_verdict(&entries, "essentials.fly"), Some(true));
    }

    fn user(groups: &[&str], perms: &[&str]) -> PUser {
        PUser {
            name: "Steve".into(),
            groups: groups.iter().map(|g| (*g).to_string()).collect(),
            permissions: perms.iter().map(|p| (*p).to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn inheritance_and_user_override() {
        let mut store = Store::default();
        store.users.insert("a".into(), user(&["moderator"], &[]));
        // moderator -> vip -> default; vip grants chat.color, moderator grants fly.
        assert_eq!(check_in(&store, "a", "essentials:chat.color"), Some(true));
        assert_eq!(check_in(&store, "a", "essentials:fly"), Some(true));
        // Nobody granted this one, so the server default must decide.
        assert_eq!(check_in(&store, "a", "essentials:gmc"), None);
        // A user-level negation beats anything a group says.
        store.users.get_mut("a").unwrap().permissions.push("-essentials.fly".into());
        assert_eq!(check_in(&store, "a", "essentials:fly"), Some(false));
    }

    #[test]
    fn admin_star_and_unknown_users() {
        let mut store = Store::default();
        store.users.insert("boss".into(), user(&["admin"], &[]));
        assert_eq!(check_in(&store, "boss", "minecraft:command.stop"), Some(true));
        // A player we've never stored still resolves through the default group.
        assert_eq!(check_in(&store, "stranger", "essentials:fly"), None);
    }

    #[test]
    fn weight_decides_between_groups() {
        let mut store = Store::default();
        store.groups.get_mut("vip").unwrap().permissions = vec!["essentials.fly".into()];
        store.groups.get_mut("moderator").unwrap().permissions = vec!["-essentials.fly".into()];
        store.users.insert("a".into(), user(&["vip", "moderator"], &[]));
        // moderator outweighs vip, so its denial is consulted first.
        assert_eq!(check_in(&store, "a", "essentials:fly"), Some(false));
    }

    #[test]
    fn expired_membership_is_ignored() {
        let mut store = Store::default();
        let mut u = user(&["admin"], &[]);
        u.expiries.insert("admin".into(), 1); // long in the past
        store.users.insert("a".into(), u);
        assert_eq!(check_in(&store, "a", "minecraft:command.stop"), None);
        let groups = active_groups(&store, store.users.get("a"));
        assert_eq!(groups, vec!["default".to_string()]);
    }

    #[test]
    fn inheritance_cycles_terminate() {
        let mut store = Store::default();
        store.groups.get_mut("default").unwrap().inherits = vec!["admin".into()];
        store.users.insert("a".into(), user(&["admin"], &[]));
        assert_eq!(check_in(&store, "a", "anything"), Some(true));
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90s"), Some(90));
        assert_eq!(parse_duration("2h"), Some(7200));
        assert_eq!(parse_duration("1w"), Some(604_800));
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("10x"), None);
    }
}
