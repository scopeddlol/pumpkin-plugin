//! Pumpkin Essentials — an essentials suite, PermissionsEx-style permissions, economy, market and
//! chat formatting in a single WebAssembly plugin.

mod cmds;
mod data;
mod events;
mod perms;
mod tp;
mod util;

use pumpkin_plugin_api::{Context, Plugin, PluginMetadata, permissions, register_plugin};

struct Essentials;

impl Plugin for Essentials {
    fn new() -> Self {
        Self
    }

    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            // Must stay equal to `util::NS`: the server namespaces permission nodes with it.
            name: util::NS.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            authors: vec!["Pumpkin Essentials contributors".into()],
            description: "Essentials commands, PermissionsEx-style permissions, economy, market and chat formatting."
                .into(),
            dependencies: vec![],
            permissions: vec![permissions::FS_WRITE_DATA.into()],
        }
    }

    fn on_load(&self, context: Context) -> Result<(), String> {
        data::init_dir(&context.get_data_folder());
        data::load_config();
        data::load_data();
        perms::load();
        cmds::market::load();

        events::register(&context)?;
        cmds::basic::register(&context);
        cmds::teleport::register(&context);
        cmds::economy::register(&context);
        cmds::market::register(&context);
        cmds::pex::register(&context);
        cmds::misc::register(&context);

        events::refresh_tabs(&context.get_server());
        tracing::info!("Essentials {} loaded", env!("CARGO_PKG_VERSION"));
        Ok(())
    }

    fn on_unload(&self, _context: Context) -> Result<(), String> {
        data::save_data();
        Ok(())
    }
}

register_plugin!(Essentials);
