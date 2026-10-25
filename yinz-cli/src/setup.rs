use clap::{Args, CommandFactory};
use clap_complete::Shell;
use log::info;
use serde::{Deserialize, Serialize};
use std::{fmt::Write as FmtWrite, fs, io::Write, path::PathBuf, process};
use yinz_config::assets::*;
use yinz_runtime::logging::startup_stage;
#[allow(unused_imports)]
use {
    crate::args::CliArgs, crate::args::Command, crate::args::SessionCommand, crate::args::Sessions,
    yinz_config::config::Config, yinz_config::config::ConfigError, yinz_config::consts::FEATURES,
    yinz_config::consts::YINZ_DEFAULT_THEMES, yinz_runtime::consts::VERSION,
    yinz_runtime::consts::YINZ_CACHE_DIR, yinz_runtime::errors::prelude::*, yinz_runtime::home::*,
    yinz_types::data::LayoutInfo, yinz_types::input::layout::Layout,
    yinz_types::input::options::Options,
};

const CONFIG_NAME: &str = "config.kdl";
static ARROW_SEPARATOR: &str = "";

pub fn dump_asset(asset: &[u8]) -> std::io::Result<()> {
    std::io::stdout().write_all(asset)?;
    Ok(())
}

pub const FISH_EXTRA_COMPLETION: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/completions/comp.fish"
));

pub const BASH_EXTRA_COMPLETION: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/completions/comp.bash"
));

pub const ZSH_EXTRA_COMPLETION: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/completions/comp.zsh"
));

pub const BASH_AUTO_START_SCRIPT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/shell/auto-start.bash"
));

pub const FISH_AUTO_START_SCRIPT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/shell/auto-start.fish"
));

pub const ZSH_AUTO_START_SCRIPT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/shell/auto-start.zsh"
));

pub fn dump_default_config() -> std::io::Result<()> {
    dump_asset(DEFAULT_CONFIG)
}

pub fn dump_specified_layout(layout: &str) -> std::io::Result<()> {
    match layout {
        "strider" => dump_asset(STRIDER_LAYOUT),
        "default" => dump_asset(DEFAULT_LAYOUT),
        "compact" => dump_asset(COMPACT_BAR_LAYOUT),
        "disable-status" => dump_asset(NO_STATUS_LAYOUT),
        "classic" => dump_asset(CLASSIC_LAYOUT),
        custom => {
            info!("Dump {custom} layout");
            let custom = add_layout_ext(custom);
            let home = default_layout_dir();
            let path = home.map(|h| h.join(&custom));
            let layout_exists = path.as_ref().map(|p| p.exists()).unwrap_or_default();

            match (path, layout_exists) {
                (Some(path), true) => {
                    let content = fs::read_to_string(path)?;
                    std::io::stdout().write_all(content.as_bytes())
                },
                _ => {
                    log::error!("No layout named {custom} found");
                    Ok(())
                },
            }
        },
    }
}

pub fn dump_specified_swap_layout(swap_layout: &str) -> std::io::Result<()> {
    match swap_layout {
        "strider" => dump_asset(STRIDER_SWAP_LAYOUT),
        "default" => dump_asset(DEFAULT_SWAP_LAYOUT),
        "compact" => dump_asset(COMPACT_BAR_SWAP_LAYOUT),
        "classic" => dump_asset(CLASSIC_SWAP_LAYOUT),
        not_found => Err(std::io::Error::other(format!(
            "Swap Layout not found for: {}",
            not_found
        ))),
    }
}

#[derive(Debug, Default, Clone, Args, Serialize, Deserialize)]
pub struct Setup {
    /// Dump the default configuration file to stdout
    #[clap(long, value_parser)]
    pub dump_config: bool,

    /// Disables loading of configuration file at default location,
    /// loads the defaults that yinz ships with
    #[clap(long, value_parser)]
    pub clean: bool,

    /// Checks the configuration of yinz and displays
    /// currently used directories
    #[clap(long, value_parser)]
    pub check: bool,

    /// Dump specified layout to stdout
    #[clap(long, value_parser)]
    pub dump_layout: Option<String>,

    /// Dump the specified swap layout file to stdout
    #[clap(long, value_parser)]
    pub dump_swap_layout: Option<String>,

    /// Generates completion for the specified shell
    #[clap(long, value_name = "SHELL", value_parser)]
    pub generate_completion: Option<String>,

    /// Generates auto-start script for the specified shell
    #[clap(long, value_name = "SHELL", value_parser)]
    pub generate_auto_start: Option<String>,
}
pub fn load_config(args: &CliArgs) -> Result<Config, ConfigError> {
    let clean = matches!(&args.command, Some(Command::Setup(setup)) if setup.clean);
    Config::load(args.config.as_deref(), args.config_dir.as_deref(), clean)
}

impl Setup {
    /// Load terminal configuration and CLI overrides without reading a session layout.
    pub fn config_without_layout(cli_args: &CliArgs) -> Result<(Config, Options), ConfigError> {
        let mut config = load_config(cli_args)?;
        let options = yinz_config::config::apply_themes(
            &mut config,
            merge_attach_command_options(cli_args.options(), cli_args),
            cli_args.config_dir.clone(),
        )?;
        Ok((config, options))
    }

    /// Entrypoint from main
    /// Merges options from the config file and the command line options
    /// into `[Options]`, the command line options superseding the layout
    /// file options, superseding the config file options:
    /// 1. command line options (`yinz options`)
    /// 2. layout options
    ///    (`layout.kdl` / `yinz --layout`)
    /// 3. config options (`config.kdl`)
    pub fn from_cli_args(
        cli_args: &CliArgs,
    ) -> Result<(Config, Option<LayoutInfo>, Options, Config, Options), ConfigError> {
        // note that this can potentially exit the process
        Setup::handle_setup_commands(cli_args);
        let config = startup_stage("cli.config", || load_config(cli_args))?;
        let cli_config_options = merge_attach_command_options(cli_args.options(), cli_args);

        let mut config_without_layout = config.clone();
        let (layout_info, mut config) = startup_stage("cli.layout", || {
            yinz_config::config::load_layout_and_config(
                cli_config_options.as_ref(),
                config,
                cli_args.config_dir.clone(),
                cli_args.config.as_deref(),
                cli_args.layout_string.as_deref(),
                cli_args.layout.as_deref(),
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            )
        })?;

        let config_options = startup_stage("cli.themes", || {
            yinz_config::config::apply_themes(
                &mut config,
                cli_config_options.clone(),
                cli_args.config_dir.clone(),
            )
        })?;
        let config_options_without_layout = startup_stage("cli.themes_without_layout", || {
            yinz_config::config::apply_themes(
                &mut config_without_layout,
                cli_config_options,
                cli_args.config_dir.clone(),
            )
        })?;

        if let Some(Command::Setup(setup)) = &cli_args.command {
            setup
                .from_cli_with_options(cli_args, &config_options)
                .map_or_else(
                    |e| {
                        eprintln!("{:?}", e);
                        process::exit(1);
                    },
                    |_| {},
                );
        };
        Ok((
            config,
            layout_info,
            config_options,
            config_without_layout,
            config_options_without_layout,
        ))
    }

    /// General setup helpers
    pub fn from_cli(&self) -> Result<()> {
        if self.clean {
            return Ok(());
        }

        if self.dump_config {
            dump_default_config()?;
            std::process::exit(0);
        }

        if let Some(shell) = &self.generate_completion {
            Self::generate_completion(shell);
            std::process::exit(0);
        }

        if let Some(shell) = &self.generate_auto_start {
            Self::generate_auto_start(shell);
            std::process::exit(0);
        }

        if let Some(layout) = &self.dump_layout {
            dump_specified_layout(layout)?;
            std::process::exit(0);
        }

        if let Some(swap_layout) = &self.dump_swap_layout {
            dump_specified_swap_layout(swap_layout)?;
            std::process::exit(0);
        }

        Ok(())
    }

    /// Checks the merged configuration
    pub fn from_cli_with_options(&self, opts: &CliArgs, config_options: &Options) -> Result<()> {
        if self.check {
            Setup::check_defaults_config(opts, config_options)?;
            std::process::exit(0);
        }

        Ok(())
    }

    pub fn check_defaults_config(opts: &CliArgs, config_options: &Options) -> std::io::Result<()> {
        let data_dir = opts.data_dir.clone().unwrap_or_else(get_default_data_dir);
        let config_dir = opts.config_dir.clone().or_else(find_default_config_dir);
        let layout_dir = config_options
            .layout_dir
            .clone()
            .or_else(|| get_layout_dir(config_dir.clone()));
        // The same helper the session uses, so this reports the folder plugins are actually
        // loaded from - absolutized included.
        let user_plugin_dir =
            user_plugin_dir_or_default(config_options.user_plugin_dir.clone(), config_dir.clone());
        let system_data_dir = system_data_dir();
        let config_file =
            Config::config_file_path(opts.config.as_deref(), opts.config_dir.as_deref());

        let mut message = String::new();

        writeln!(&mut message, "[Version]: {:?}", VERSION).unwrap();
        if let Some(config_dir) = config_dir {
            writeln!(&mut message, "[CONFIG DIR]: \"{}\"", config_dir.display()).unwrap();
        } else {
            message.push_str("[CONFIG DIR]: Not Found\n");
            let mut default_config_dirs = default_config_dirs()
                .iter()
                .filter_map(|p| p.clone())
                .collect::<Vec<PathBuf>>();
            default_config_dirs.dedup();
            message.push_str(
                " On your system yinz looks in the following config directories by default:\n",
            );
            for dir in default_config_dirs {
                writeln!(&mut message, " \"{}\"", dir.display()).unwrap();
            }
        }
        if let Some(config_file) = config_file {
            writeln!(
                &mut message,
                "[LOOKING FOR CONFIG FILE FROM]: \"{}\"",
                config_file.display()
            )
            .unwrap();
            match Config::from_path(&config_file, None) {
                Ok(_) => message.push_str("[CONFIG FILE]: Well defined.\n"),
                Err(e) => writeln!(
                    &mut message,
                    "[CONFIG ERROR]: {}. \n By default, yinz loads default configuration",
                    e
                )
                .unwrap(),
            }
        } else {
            message.push_str("[CONFIG FILE]: Not Found\n");
            writeln!(
                &mut message,
                " By default yinz looks for [{}], in the configuration directory",
                CONFIG_NAME,
            )
            .unwrap();
        }
        writeln!(&mut message, "[CACHE DIR]: {}", YINZ_CACHE_DIR.display()).unwrap();
        writeln!(&mut message, "[DATA DIR]: \"{}\"", data_dir.display()).unwrap();
        message.push_str("[BUILTIN PLUGINS]: Embedded Scheme sources\n");
        if let Some(layout_dir) = layout_dir {
            writeln!(&mut message, "[LAYOUT DIR]: \"{}\"", layout_dir.display()).unwrap();
        } else {
            message.push_str("[LAYOUT DIR]: Not Found\n");
        }
        // Reported whether or not it exists, as the layout dir is: the path is the useful
        // answer, and nothing creates this folder until something writes a plugin into it.
        if let Some(user_plugin_dir) = user_plugin_dir {
            writeln!(
                &mut message,
                "[USER PLUGIN DIR]: \"{}\"",
                user_plugin_dir.display()
            )
            .unwrap();
        } else {
            message.push_str("[USER PLUGIN DIR]: Not Found\n");
        }
        writeln!(
            &mut message,
            "[SYSTEM DATA DIR]: \"{}\"",
            system_data_dir.display()
        )
        .unwrap();

        writeln!(&mut message, "[ARROW SEPARATOR]: {}", ARROW_SEPARATOR).unwrap();
        message.push_str(" Is the [ARROW_SEPARATOR] displayed correctly?\n");
        message.push_str(" If not you may want to either start yinz with a compatible mode: 'yinz options --simplified-ui true'\n");
        message.push_str(" Or use a font that includes this glyph, such as a Nerd Font.\n");
        message.push_str("[MOUSE INTERACTION]: \n");
        message.push_str(" Can be temporarily disabled through pressing the [SHIFT] key.\n");
        message.push_str(" If that doesn't fix any issues consider to disable the mouse handling of yinz: 'yinz options --disable-mouse-mode'\n");

        let default_editor = std::env::var("EDITOR")
            .or_else(|_| std::env::var("VISUAL"))
            .unwrap_or_else(|_| String::from("Not set, checked $EDITOR and $VISUAL"));
        writeln!(&mut message, "[DEFAULT EDITOR]: {}", default_editor).unwrap();
        writeln!(&mut message, "[FEATURES]: {:?}", FEATURES).unwrap();

        std::io::stdout().write_all(message.as_bytes())?;

        Ok(())
    }
    fn generate_completion(shell: &str) {
        let shell = match shell.to_lowercase().as_str() {
            "bash" => Shell::Bash,
            "elvish" => Shell::Elvish,
            "fish" => Shell::Fish,
            "zsh" => Shell::Zsh,
            _ => {
                eprintln!("Unsupported shell: {}", shell);
                std::process::exit(1);
            },
        };
        let mut out = std::io::stdout();
        clap_complete::generate(shell, &mut CliArgs::command(), "yinz", &mut out);
        // add shell dependent extra completion
        match shell {
            Shell::Bash => {
                let _ = out.write_all(BASH_EXTRA_COMPLETION);
            },
            Shell::Elvish => {},
            Shell::Fish => {
                let _ = out.write_all(FISH_EXTRA_COMPLETION);
            },
            Shell::Zsh => {
                let _ = out.write_all(ZSH_EXTRA_COMPLETION);
            },
            _ => {},
        };
    }

    fn generate_auto_start(shell: &str) {
        let shell: Shell = match shell.to_lowercase().parse() {
            Ok(shell) => shell,
            _ => {
                eprintln!("Unsupported shell: {}", shell);
                std::process::exit(1);
            },
        };

        let mut out = std::io::stdout();
        match shell {
            Shell::Bash => {
                let _ = out.write_all(BASH_AUTO_START_SCRIPT);
            },
            Shell::Fish => {
                let _ = out.write_all(FISH_AUTO_START_SCRIPT);
            },
            Shell::Zsh => {
                let _ = out.write_all(ZSH_AUTO_START_SCRIPT);
            },
            _ => {},
        }
    }
    fn handle_setup_commands(cli_args: &CliArgs) {
        if let Some(Command::Setup(setup)) = &cli_args.command {
            setup.from_cli().map_or_else(
                |e| {
                    eprintln!("{:?}", e);
                    process::exit(1);
                },
                |_| {},
            );
        };
    }
}

fn merge_attach_command_options(
    cli_config_options: Option<Options>,
    cli_args: &CliArgs,
) -> Option<Options> {
    if let Some(Command::Sessions(Sessions::Attach { options, .. })) = cli_args.command.clone() {
        match options.clone().as_deref() {
            Some(SessionCommand::Options(options)) => match cli_config_options {
                Some(cli_config_options) => Some(crate::options::merge_from_cli(
                    &cli_config_options,
                    options.to_owned().into(),
                )),
                None => Some(options.to_owned().into()),
            },
            _ => cli_config_options,
        }
    } else {
        cli_config_options
    }
}

#[cfg(test)]
#[path = "unit/setup_setup_test.rs"]
mod setup_test;
