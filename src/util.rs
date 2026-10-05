//! Shared helpers: pretty messages, command registration, argument parsing, time and money.

use pumpkin_plugin_api::{
    Context, Player, Server,
    command::{
        Arg, ArgumentType, CommandError, CommandNode, CommandSender, CommandSuggestion,
        CommandSuggestions, ConsumedArgs, StringType, SuggestionRequest,
    },
    commands::{CommandHandler, CommandSuggestionHandler},
    permission::{Permission, PermissionDefault, PermissionLevel},
    text::TextComponent,
};
use std::time::{SystemTime, UNIX_EPOCH};

/// Namespace used for every permission node this plugin registers.
/// The server prefixes nodes with the plugin name, so keep the two equal.
pub const NS: &str = "essentials";

// ---------------------------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------------------------

/// Parses `&`-style legacy colour codes into a text component.
pub fn colored(text: &str) -> TextComponent {
    TextComponent::from_legacy_string_with_code(text, '&')
}

/// `&8» ` brand prefix followed by a colour-coded body.
fn prefixed(color: &str, text: &str) -> TextComponent {
    colored(&format!("&8&l» {color}{text}"))
}

pub fn ok(text: &str) -> TextComponent {
    prefixed("&a", text)
}

pub fn info(text: &str) -> TextComponent {
    prefixed("&7", text)
}

pub fn warn(text: &str) -> TextComponent {
    prefixed("&e", text)
}

pub fn bad(text: &str) -> TextComponent {
    prefixed("&c", text)
}

/// Strips `&x` colour codes so user-provided strings can be compared or stored plain.
pub fn strip_codes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if (c == '&' || c == '§') && chars.peek().is_some_and(|n| n.is_ascii_alphanumeric()) {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// Concatenate components into one line.
pub fn join(parts: Vec<TextComponent>) -> TextComponent {
    parts
        .into_iter()
        .fold(TextComponent::text(""), |acc, part| acc.add_child(part))
}

pub fn tell(player: &Player, text: TextComponent) {
    player.send_system_message(text, false);
}

// ---------------------------------------------------------------------------------------------
// Command plumbing
// ---------------------------------------------------------------------------------------------

pub type CmdResult = Result<i32, CommandError>;

/// Everything a command body needs, bundled so handlers are plain `fn`s.
pub struct Cx {
    pub sender: CommandSender,
    pub server: Server,
    pub args: ConsumedArgs,
}

impl Cx {
    /// The invoking player, or a friendly error when run from the console.
    pub fn player(&self) -> Result<Player, CommandError> {
        self.sender
            .as_player()
            .ok_or_else(|| fail("This command can only be used by players."))
    }

    /// Reply to whoever ran the command.
    pub fn reply(&self, text: TextComponent) {
        self.sender.send_message(text);
    }

    pub fn ok(&self, text: &str) -> CmdResult {
        self.reply(ok(text));
        Ok(1)
    }

    pub fn info(&self, text: &str) -> CmdResult {
        self.reply(info(text));
        Ok(1)
    }

    /// A single-word / greedy string argument; `None` when it was not supplied.
    pub fn arg(&self, key: &str) -> Option<String> {
        match self.args.get_value(key) {
            Arg::Simple(s) | Arg::Msg(s) if !s.is_empty() => Some(s),
            _ => None,
        }
    }

    pub fn require(&self, key: &str, usage: &str) -> Result<String, CommandError> {
        self.arg(key).ok_or_else(|| fail(&format!("Usage: {usage}")))
    }

    /// Resolve an online player by (case-insensitive) name.
    pub fn online(&self, name: &str) -> Result<Player, CommandError> {
        find_online(&self.server, name)
            .ok_or_else(|| fail(&format!("Player '{name}' is not online.")))
    }

    /// Optional target: the named online player, or the sender when no name was supplied.
    pub fn target_or_self(&self, key: &str) -> Result<Player, CommandError> {
        match self.arg(key) {
            Some(name) => self.online(&name),
            None => self.player(),
        }
    }

    pub fn has_perm(&self, node: &str) -> bool {
        self.sender.has_permission(&self.server, &format!("{NS}:{node}"))
    }
}

pub fn fail(message: &str) -> CommandError {
    CommandError::CommandFailed(bad(message))
}

pub fn find_online(server: &Server, name: &str) -> Option<Player> {
    server
        .get_all_players()
        .into_iter()
        .find(|p| p.get_name().eq_ignore_ascii_case(name))
}

type Handler = fn(&Cx) -> CmdResult;

struct FnHandler(Handler);

impl CommandHandler for FnHandler {
    fn handle(
        &self,
        sender: CommandSender,
        server: Server,
        args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        (self.0)(&Cx {
            sender,
            server,
            args,
        })
    }
}

/// Wrap a plain function as a command handler.
pub fn run(handler: Handler) -> impl CommandHandler + 'static {
    FnHandler(handler)
}

pub fn lit(name: &str) -> CommandNode {
    CommandNode::literal(name)
}

/// A single-word string argument.
pub fn word(name: &str) -> CommandNode {
    CommandNode::argument(name, &ArgumentType::String(StringType::SingleWord))
}

/// A string argument swallowing the rest of the line.
pub fn greedy(name: &str) -> CommandNode {
    CommandNode::argument(name, &ArgumentType::String(StringType::Greedy))
}

/// A single-word argument that tab-completes online player names.
pub fn player_arg(name: &str) -> CommandNode {
    word(name).suggest(PlayerNames)
}

/// Suggests the names of online players.
pub struct PlayerNames;

impl CommandSuggestionHandler for PlayerNames {
    fn suggest(
        &self,
        _sender: CommandSender,
        server: Server,
        request: SuggestionRequest,
    ) -> CommandSuggestions {
        let prefix = request.remaining.to_lowercase();
        suggestions(
            &request,
            server
                .get_all_players()
                .iter()
                .map(Player::get_name)
                .filter(|n| n.to_lowercase().starts_with(&prefix))
                .collect(),
        )
    }
}

/// Build a suggestion reply covering the argument text being typed.
pub fn suggestions(request: &SuggestionRequest, values: Vec<String>) -> CommandSuggestions {
    CommandSuggestions {
        start: request.start,
        length: request.remaining.len() as u32,
        values: values
            .into_iter()
            .map(|value| CommandSuggestion {
                value,
                tooltip: None,
            })
            .collect(),
    }
}

#[derive(Clone, Copy)]
pub enum Access {
    /// Everybody may use it by default (PEX groups can still revoke it).
    Everyone,
    /// Operators of at least this level only, unless a permission group grants it.
    Op(PermissionLevel),
}

/// Registers `node` with the server (so it has a default) and returns the node string for
/// `Context::register_command`.
pub fn register_node(ctx: &Context, node: &str, description: &str, access: Access) -> String {
    let default = match access {
        Access::Everyone => PermissionDefault::Allow,
        Access::Op(level) => PermissionDefault::Op(level),
    };
    // Re-registering a node (e.g. shared between aliases) is harmless; ignore the error.
    let _ = ctx.register_permission(&Permission {
        node: format!("{NS}:{node}"),
        description: description.to_string(),
        default,
        children: Vec::new(),
    });
    format!("{NS}:{node}")
}

/// Register a command (first name primary, the rest aliases) guarded by `{NS}:{node}`.
pub fn command(
    ctx: &Context,
    names: &[&str],
    description: &str,
    node: &str,
    access: Access,
    build: impl FnOnce(pumpkin_plugin_api::commands::Command) -> pumpkin_plugin_api::commands::Command,
) {
    let perm = register_node(ctx, node, description, access);
    let names: Vec<String> = names.iter().map(|n| (*n).to_string()).collect();
    let cmd = pumpkin_plugin_api::commands::Command::new(&names, description);
    ctx.register_command(build(cmd), &perm);
}

// ---------------------------------------------------------------------------------------------
// Time & money
// ---------------------------------------------------------------------------------------------

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `3725` -> `1h 2m 5s`
pub fn fmt_duration(mut secs: u64) -> String {
    if secs == 0 {
        return "0s".into();
    }
    let mut parts = Vec::new();
    for (unit, label) in [(86_400, "d"), (3_600, "h"), (60, "m"), (1, "s")] {
        if secs >= unit {
            parts.push(format!("{}{label}", secs / unit));
            secs %= unit;
        }
    }
    parts.join(" ")
}

/// Money is stored as integer cents to avoid floating point drift.
pub type Cents = i64;

/// Parses `12`, `12.5`, `1.5k`, `2m`, `1b` into cents. Rejects zero/negative unless `allow_zero`.
pub fn parse_money(input: &str) -> Option<Cents> {
    let s = input.trim().replace(',', "").to_lowercase();
    let (num, mult) = match s.chars().last()? {
        'k' => (&s[..s.len() - 1], 1_000.0),
        'm' => (&s[..s.len() - 1], 1_000_000.0),
        'b' => (&s[..s.len() - 1], 1_000_000_000.0),
        _ => (s.as_str(), 1.0),
    };
    let value: f64 = num.parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let cents = (value * mult * 100.0).round();
    (cents <= 9.0e15).then_some(cents as Cents)
}

/// `123456` -> `1,234.56`
pub fn fmt_amount(cents: Cents) -> String {
    let neg = cents < 0;
    let cents = cents.unsigned_abs();
    let whole = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let frac = cents % 100;
    let sign = if neg { "-" } else { "" };
    if frac == 0 {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{frac:02}")
    }
}

/// Title-cases `diamond_sword` -> `Diamond Sword`.
pub fn pretty_item(registry_key: &str) -> String {
    registry_key
        .rsplit(':')
        .next()
        .unwrap_or(registry_key)
        .split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or_else(String::new, |f| {
                f.to_uppercase().collect::<String>() + c.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_parsing() {
        assert_eq!(parse_money("50"), Some(5_000));
        assert_eq!(parse_money("12.5"), Some(1_250));
        assert_eq!(parse_money("1.5k"), Some(150_000));
        assert_eq!(parse_money("2M"), Some(200_000_000));
        assert_eq!(parse_money("1,000"), Some(100_000));
        assert_eq!(parse_money("-5"), None);
        assert_eq!(parse_money("abc"), None);
        assert_eq!(parse_money("NaN"), None);
        assert_eq!(parse_money(""), None);
        assert_eq!(parse_money("1e30"), None);
    }

    #[test]
    fn money_formatting() {
        assert_eq!(fmt_amount(0), "0");
        assert_eq!(fmt_amount(5), "0.05");
        assert_eq!(fmt_amount(123_456), "1,234.56");
        assert_eq!(fmt_amount(100_000_000), "1,000,000");
        assert_eq!(fmt_amount(-250), "-2.50");
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(fmt_duration(0), "0s");
        assert_eq!(fmt_duration(3_725), "1h 2m 5s");
        assert_eq!(fmt_duration(86_400), "1d");
    }

    #[test]
    fn colour_codes_are_stripped() {
        assert_eq!(strip_codes("&6Hello &lworld"), "Hello world");
        assert_eq!(strip_codes("fish & chips"), "fish & chips");
    }

    #[test]
    fn item_names() {
        assert_eq!(pretty_item("minecraft:diamond_sword"), "Diamond Sword");
        assert_eq!(pretty_item("tnt"), "Tnt");
    }
}
