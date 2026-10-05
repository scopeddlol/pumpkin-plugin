# Pumpkin Essentials

An all-in-one **essentials suite** for [Pumpkin](https://pumpkinmc.org) servers: commands, a
**PermissionsEx-style permission system**, an **economy** with a **player market GUI**, and
**chat formatting** — one WebAssembly plugin, no dependencies.

Built against the Pumpkin plugin API at commit `4426d11` (server `0.2.0+26.3-26.51`) and verified to load
on a server built from that commit. `Cargo.toml` pins the API by git `rev` because the crates.io release of
`pumpkin-plugin-api` is older than that server and is rejected at load time. If you run a different
Pumpkin build, change `rev` to your server's commit.

## Install

```bash
./build.sh /path/to/your/pumpkin-server     # builds and copies into <server>/plugins/
```

or manually:

```bash
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/pumpkin_essentials.wasm /path/to/server/plugins/
```

Restart the server. On first load Pumpkin asks you to approve the plugin's `fs.write.data` permission
(it stores its data in its own folder). To skip the prompt, add `allowed_permissions = ["fs.write.data"]`
under `[plugins]` in `config/features.toml` / `pumpkin.toml`. Config and data appear in `plugins/data/essentials/`:

| File               | What                                                        |
| ------------------ | ----------------------------------------------------------- |
| `config.json`      | currency, homes, teleport, rtp, market, chat & join formats |
| `permissions.json` | groups, users, prefixes (edit by hand or use `/pex`)        |
| `data.json`        | balances, homes, warps, spawn                               |
| `market.json`      | active market listings                                      |

`/essentials reload` re-reads everything.

## Commands

| Area      | Commands |
| --------- | -------- |
| Gamemode  | `/gmc` `/gms` `/gma` `/gmsp` `/gm <c\|s\|a\|sp> [player]` |
| Player    | `/fly [player]` `/speed <1-10>` `/heal` `/feed` `/god` `/ping` `/top` `/seen` `/suicide` |
| Homes     | `/sethome [name]` `/home [name]` `/delhome <name>` `/homes` (clickable list) |
| Warps     | `/warp [name]` `/warps` `/setwarp <name>` `/delwarp <name>` |
| Spawn     | `/spawn` `/setspawn` |
| Teleport  | `/tpa` `/tpahere` `/tpaccept` `/tpdeny` `/tpcancel` (click-to-accept buttons), `/back`, `/rtp` (alias `/wild`) |
| Economy   | `/bal [player]` `/pay <player> <amount>` `/baltop` `/eco give\|take\|set\|reset <player> <amount>` |
| Market    | `/market` (GUI) `/market sell <price>` `/market mine` `/market cancel <id>` |
| Chat      | `/msg` `/reply` `/broadcast` — plus rank-based chat formatting |
| Storage   | `/enderchest` (`/ec`) |
| Admin     | `/pex …` `/essentials [reload]` |

Amounts accept `k`/`m`/`b` suffixes (`/pay Steve 2.5k`).
Teleports have a configurable warm-up (cancelled if you move) and cooldown; `/tpa` requests expire.

### Market

Hold a stack and run `/market sell <price>` (price is for the whole stack). Browse with `/market`
and click an item to buy. Sellers are paid even when offline, minus `market_tax_percent`.
Only **unmodified** stacks can be listed (no enchants, damage, names, container contents):
listings store just an item and count, and refusing is safer than silently losing data.

## Permissions

Works like PermissionsEx: groups with inheritance, per-user overrides, wildcards and negation.

```
/pex groups
/pex group vip perm add essentials.fly
/pex group vip prefix &6[VIP] &e
/pex group vip homes 5
/pex group moderator inherit add vip
/pex user Steve group add vip 30d        # temporary rank
/pex user Steve perm add -essentials.rtp # deny
/pex check Steve essentials.fly          # debug
```

* Nodes are dotted: `essentials.fly`. The server's `essentials:fly` / `minecraft:command.gamemode`
  forms are normalised to `essentials.fly` / `minecraft.command.gamemode`, so you can also grant
  **vanilla and other plugins' commands** (e.g. `minecraft.command.gamemode`).
* Wildcards: `*`, `essentials.*`, `essentials.homes.*`. Prefix with `-` to deny.
* Resolution: user entries first, then the user's groups by `weight` (highest first), each followed by
  the groups it inherits. In a layer the most specific node wins; a denial beats a grant on a tie.
* If nothing matches, the server's own default applies (ops-only commands stay ops-only).
* Everybody is implicitly in the `default` group. Defaults ship with `default`, `vip`, `moderator`, `admin`.
* Safety net: a level-4 operator can always use `/pex`.
* Prefixes/suffixes feed both chat and the tab list. Players already online may need to reconnect
  to see updated *tab-completion* for commands.

Useful nodes: `essentials.chat.color` (use `&` codes in chat), `essentials.tp.bypass` (skip warm-up and
cooldown), `essentials.market.admin` (cancel anyone's listing), `essentials.*.others`
(`gamemode`, `heal`, `feed`, `fly`, `god`).

## Chat formatting

`config.json` → `chat_format`, default `{prefix}{name}{suffix}&8 » &r{message}`.
Placeholders: `{prefix} {suffix} {name} {group} {message}`. Join/leave/first-join messages and the
join MOTD support `{name} {prefix} {suffix} {group} {balance} {online}`. Colours use `&` codes.

## Development

```bash
cargo test --lib                                 # unit tests (permissions, money, ...)
cargo build --release --target wasm32-wasip2     # the plugin
```

Layout: `perms.rs` (engine), `data.rs` (persistence/config/runtime), `tp.rs` (teleport helpers),
`events.rs`, `cmds/*` (one file per command family).
