use super::*;
use serde_json::{json, Value};
use syntect::highlighting::{Color, Theme, ThemeSet};

use yinz_types::codex_verified_version as verified_version;

pub(super) const PROVIDER: Provider = Provider {
    version,
    verified: verified_version!(),
    resolve,
};

fn version(line: &str) -> Option<&str> {
    release(line.strip_prefix("codex-cli ")?)
}

pub(super) fn resolve(context: &LaunchContext, snapshot: &mut AgentThemeSnapshot) -> Result<()> {
    let (cwd, overrides, profile) = launch_options(context)?;
    let root = context
        .environment
        .get(OsStr::new("CODEX_HOME"))
        .map(PathBuf::from)
        .unwrap_or(context.home()?.join(".codex"));
    let result = read_configuration(context, &cwd, &overrides)?;
    let mut theme = result["config"]["tui"]["theme"].as_str().map(str::to_owned);
    if let Some(profile) = profile {
        // 0.162.1 uses profile files, not the historical [profiles.name] merge. The
        // app-server config/read API does not accept --profile. Insert its theme only
        // below enabled project and session layers, exactly as this release's loader does.
        if profile.is_empty()
            || !profile
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            bail!("invalid Codex profile name");
        }
        let user = read_toml(&root.join("config.toml"))?.unwrap_or_default();
        if user.get("profile").and_then(toml::Value::as_str) == Some(&profile)
            || user.get("profiles").and_then(|v| v.get(&profile)).is_some()
        {
            bail!("Codex profile conflicts with legacy profile configuration");
        }
        let path = root.join(format!("{profile}.config.toml"));
        let profile = read_toml(&path)?.context("selected Codex profile file is missing")?;
        if [
            "projects",
            "project_root_markers",
            "features",
            "requirements",
        ]
        .iter()
        .any(|key| profile.contains_key(*key))
        {
            bail!("Codex profile changes project discovery or policy; theme resolution is unavailable");
        }
        if let Some(value) = profile.get("tui").and_then(|v| v.get("theme")) {
            let high_priority_theme = result["layers"]
                .as_array()
                .context("Codex config layers missing")?
                .iter()
                .any(|layer| {
                    layer["disabledReason"].is_null()
                        && matches!(
                            layer["name"]["type"].as_str(),
                            Some(
                                "project"
                                    | "sessionFlags"
                                    | "legacyManagedConfigTomlFromFile"
                                    | "legacyManagedConfigTomlFromMdm"
                            )
                        )
                        && layer["config"]["tui"]["theme"].is_string()
                });
            if !high_priority_theme {
                theme = Some(
                    value
                        .as_str()
                        .context("Codex profile theme must be a string")?
                        .into(),
                );
            }
        }
    }
    snapshot.provenance.push(
        concat!(
            "codex-rust-v",
            verified_version!(),
            "/two-face-0.5.1/syntect-5.3.0"
        )
        .into(),
    );
    provider_colors(&mut snapshot.definitions);
    terminal_defaults(&mut snapshot.definitions);
    let Some(theme) = theme else {
        snapshot.theme = "automatic".into();
        snapshot.availability = Availability::Partial("Codex automatic syntax palette is unresolved; fixed provider colors are mapped and terminal-derived UI colors are mapped after successful terminal replies".into());
        return Ok(());
    };
    snapshot.theme = theme.clone();
    let parsed = if let Some(builtin) = builtin(&theme) {
        two_face::theme::extra().get(builtin).clone()
    } else {
        let path = root.join("themes").join(format!("{theme}.tmTheme"));
        snapshot.provenance.push(path.display().to_string());
        if path.try_exists().context("locate selected Codex theme")? {
            let file = std::fs::File::open(&path).context("open selected Codex tmTheme")?;
            let mut reader = std::io::BufReader::new(file.take(1024 * 1024));
            ThemeSet::load_from_reader(&mut reader).context("parse selected Codex tmTheme")?
        } else {
            model_theme(&theme)?.context("selected Codex theme is unavailable")?
        }
    };
    definitions(&parsed, &mut snapshot.definitions);
    snapshot.availability = Availability::Partial("theme scopes and fixed provider colors resolved; terminal-derived surfaces, text, accents and selection are mapped after successful terminal replies".into());
    Ok(())
}

// Palette data normalized from rust-v0.162.1/tui/assets/themes. Syntax tokens
// retain their source definitions; compilation gives shared UI colors priority.
fn model_theme(name: &str) -> Result<Option<Theme>> {
    let descriptors: BTreeMap<String, Vec<Value>> = serde_json::from_str(include_str!(concat!(
        "descriptors/codex-",
        verified_version!(),
        ".json"
    )))
    .context("read verified Codex model palettes")?;
    let Some(items) = descriptors.get(name) else {
        return Ok(None);
    };
    let mut theme = Theme::default();
    for item in items {
        let parse = |key: &str| -> Result<Option<Color>> {
            item["settings"][key]
                .as_str()
                .map(str::parse)
                .transpose()
                .context("read model palette color")
        };
        let foreground = parse("foreground")?;
        let background = parse("background")?;
        if let Some(scope) = item["scope"].as_str() {
            theme.scopes.push(syntect::highlighting::ThemeItem {
                scope: scope.parse().context("read model palette scope")?,
                style: syntect::highlighting::StyleModifier {
                    foreground,
                    background,
                    font_style: None,
                },
            });
        } else {
            theme.settings.foreground = foreground;
            theme.settings.background = background;
        }
    }
    Ok(Some(theme))
}

fn read_toml(path: &Path) -> Result<Option<toml::Table>> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("read Codex theme configuration"),
    };
    toml::from_str(&source)
        .map_err(|_| anyhow!("malformed Codex theme configuration"))
        .map(Some)
}

pub(super) fn launch_options(
    context: &LaunchContext,
) -> Result<(PathBuf, Vec<String>, Option<String>)> {
    let mut cwd = context.cwd.clone();
    let mut overrides = Vec::new();
    let mut profile = None;
    let mut args = context.arguments.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        match arg.as_str() {
            "--remote" | "--remote-auth-token-env" => {
                bail!("externally supplied Codex backend configuration cannot be resolved locally")
            },
            "-c" | "--config" => overrides.push(
                args.next()
                    .context("missing Codex config override")?
                    .clone(),
            ),
            "-p" | "--profile" => {
                profile = Some(args.next().context("missing Codex profile")?.clone())
            },
            "-C" | "--cd" => {
                cwd = context
                    .cwd
                    .join(args.next().context("missing Codex working directory")?)
            },
            _ => {
                if let Some(value) = arg
                    .strip_prefix("--config=")
                    .or_else(|| arg.strip_prefix("-c="))
                    .or_else(|| arg.strip_prefix("-c").filter(|v| !v.is_empty()))
                {
                    overrides.push(value.into());
                }
                if let Some(value) = arg
                    .strip_prefix("--profile=")
                    .or_else(|| arg.strip_prefix("-p="))
                    .or_else(|| arg.strip_prefix("-p").filter(|v| !v.is_empty()))
                {
                    profile = Some(value.into());
                }
                if let Some(value) = arg
                    .strip_prefix("--cd=")
                    .or_else(|| arg.strip_prefix("-C="))
                    .or_else(|| arg.strip_prefix("-C").filter(|v| !v.is_empty()))
                {
                    cwd = context.cwd.join(value);
                }
                if arg.starts_with("--remote=") {
                    bail!("externally supplied Codex backend configuration cannot be resolved locally");
                }
            },
        }
    }
    Ok((
        cwd.canonicalize()
            .context("resolve final Codex directory")?,
        overrides,
        profile,
    ))
}

/// Ask the pinned provider to resolve machine policy, environment roots, CLI TOML and
/// trusted project layers. This deliberately does not implement a generic TOML merge.
fn read_configuration(context: &LaunchContext, cwd: &Path, overrides: &[String]) -> Result<Value> {
    use std::io::{BufRead, BufReader};
    let mut command = context.command();
    command.arg("app-server");
    for value in overrides {
        command.args(["-c", value]);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("start Codex configuration reader")?;
    let result = (|| {
        let mut stdin = child.stdin.take().context("Codex config reader stdin")?;
        let stdout = child.stdout.take().context("Codex config reader stdout")?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout.take(4 * 1024 * 1024)).lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    if tx.send(value).is_err() {
                        break;
                    }
                }
            }
        });
        writeln!(stdin, "{}", json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"yinz_theme_resolver","version":"1"},"capabilities":{"experimentalApi":true}}})).context("initialize Codex theme configuration reader")?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let value = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .context("Codex theme configuration reader timed out")?;
            if value["id"] == 1 {
                if value.get("error").is_some() {
                    bail!("Codex rejected theme configuration initialization");
                }
                writeln!(stdin, "{}", json!({"method":"initialized"}))
                    .context("acknowledge Codex initialization")?;
                writeln!(
                    stdin,
                    "{}",
                    json!({"id":2,"method":"config/read","params":{"cwd":cwd,"includeLayers":true}})
                )
                .context("request invocation configuration")?;
            } else if value["id"] == 2 {
                return value
                    .get("result")
                    .cloned()
                    .context("Codex could not resolve invocation configuration");
            }
        }
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn source(color: Color) -> Option<SourceColor> {
    match color.a {
        0 => Some(SourceColor::Index(color.r)),
        1 => Some(SourceColor::Default),
        _ => Some(SourceColor::Rgb(color.r, color.g, color.b)),
    }
}

fn definitions(theme: &Theme, result: &mut Vec<Definition>) {
    let settings = &theme.settings;
    for (token, channel, value, role) in [
        (
            "foreground",
            Channel::Foreground,
            settings.foreground,
            Role::Foreground,
        ),
        (
            "background",
            Channel::Background,
            settings.background,
            Role::Background,
        ),
        (
            "selectionForeground",
            Channel::Foreground,
            settings.selection_foreground,
            Role::SelectionForeground,
        ),
        (
            "selection",
            Channel::Background,
            settings.selection,
            Role::SelectionBackground,
        ),
    ] {
        if let Some(color) = value.and_then(source) {
            result.push(Definition {
                token: token.into(),
                channel,
                color,
                pair: matches!(role, Role::SelectionForeground | Role::SelectionBackground)
                    .then(|| {
                        settings
                            .selection_foreground
                            .and_then(source)
                            .zip(settings.selection.and_then(source))
                    })
                    .flatten(),
                target: Some(role.into()),
            });
        }
    }
    for (token, channel, value) in [
        ("caret", Channel::Foreground, settings.caret),
        (
            "lineHighlight",
            Channel::Background,
            settings.line_highlight,
        ),
        ("misspelling", Channel::Underline, settings.misspelling),
        (
            "minimapBorder",
            Channel::Foreground,
            settings.minimap_border,
        ),
        ("accent", Channel::Foreground, settings.accent),
        (
            "bracketContentsForeground",
            Channel::Foreground,
            settings.bracket_contents_foreground,
        ),
        (
            "bracketsForeground",
            Channel::Foreground,
            settings.brackets_foreground,
        ),
        (
            "bracketsBackground",
            Channel::Background,
            settings.brackets_background,
        ),
        (
            "tagsForeground",
            Channel::Foreground,
            settings.tags_foreground,
        ),
        ("highlight", Channel::Foreground, settings.highlight),
        (
            "findHighlight",
            Channel::Background,
            settings.find_highlight,
        ),
        (
            "findHighlightForeground",
            Channel::Foreground,
            settings.find_highlight_foreground,
        ),
        ("gutter", Channel::Background, settings.gutter),
        (
            "gutterForeground",
            Channel::Foreground,
            settings.gutter_foreground,
        ),
        (
            "selectionBorder",
            Channel::Foreground,
            settings.selection_border,
        ),
        (
            "inactiveSelection",
            Channel::Background,
            settings.inactive_selection,
        ),
        (
            "inactiveSelectionForeground",
            Channel::Foreground,
            settings.inactive_selection_foreground,
        ),
        ("guide", Channel::Foreground, settings.guide),
        ("activeGuide", Channel::Foreground, settings.active_guide),
        ("stackGuide", Channel::Foreground, settings.stack_guide),
        ("shadow", Channel::Background, settings.shadow),
    ] {
        if let Some(color) = value.and_then(source) {
            result.push(Definition {
                token: token.into(),
                channel,
                color,
                pair: None,
                target: None,
            });
        }
    }
    for item in &theme.scopes {
        for selector in &item.scope.selectors {
            let token = selector.path.to_string().trim().to_owned();
            let role = selector
                .extract_single_scope()
                .and_then(|scope| scope_role(&scope.to_string()));
            let foreground = item.style.foreground.and_then(source);
            let background = item.style.background.and_then(source);
            let pair = foreground.zip(background);
            for (channel, color) in [
                (Channel::Foreground, foreground),
                (Channel::Background, background),
            ] {
                if let Some(color) = color {
                    let target = match (role, channel) {
                        (Some(Role::Success), Channel::Background) => {
                            Some(Role::AdditionBackground)
                        },
                        (Some(Role::Error), Channel::Background) => Some(Role::RemovalBackground),
                        (role, Channel::Foreground) => role,
                        _ => None,
                    }
                    .map(Into::into);
                    result.push(Definition {
                        token: token.clone(),
                        channel,
                        color,
                        pair,
                        target,
                    });
                }
            }
        }
    }
}

fn scope_role(scope: &str) -> Option<Role> {
    Some(match scope {
        "markup.inserted" | "diff.inserted" => Role::Success,
        "markup.deleted" | "diff.deleted" => Role::Error,
        "codex.accent" | "markup.heading" | "markup.underline.link" | "markup.link" => Role::Accent,
        // Syntax-only colors remain unmapped; shared UI colors map at compilation.
        _ => return None,
    })
}

fn provider_colors(result: &mut Vec<Definition>) {
    for (token, color, role) in [
        ("status.success", SourceColor::Index(2), Role::Success),
        ("status.failure", SourceColor::Index(1), Role::Error),
        (
            "status.attention.dark",
            SourceColor::Index(3),
            Role::Warning,
        ),
    ] {
        result.push(Definition {
            token: token.into(),
            channel: Channel::Foreground,
            color,
            pair: None,
            target: Some(role.into()),
        });
    }
    for (token, color, role) in [
        (
            "diff.add.dark.rgb",
            SourceColor::Rgb(33, 58, 43),
            Role::AdditionBackground,
        ),
        (
            "diff.remove.dark.rgb",
            SourceColor::Rgb(74, 34, 29),
            Role::RemovalBackground,
        ),
        (
            "diff.add.light.rgb",
            SourceColor::Rgb(218, 251, 225),
            Role::AdditionBackground,
        ),
        (
            "diff.remove.light.rgb",
            SourceColor::Rgb(255, 235, 233),
            Role::RemovalBackground,
        ),
        (
            "diff.add.gutter.light.rgb",
            SourceColor::Rgb(172, 238, 187),
            Role::AdditionBackground,
        ),
        (
            "diff.remove.gutter.light.rgb",
            SourceColor::Rgb(255, 206, 203),
            Role::RemovalBackground,
        ),
        (
            "diff.add.dark.index",
            SourceColor::Index(22),
            Role::AdditionBackground,
        ),
        (
            "diff.remove.dark.index",
            SourceColor::Index(52),
            Role::RemovalBackground,
        ),
        (
            "diff.add.light.index",
            SourceColor::Index(194),
            Role::AdditionBackground,
        ),
        (
            "diff.remove.light.index",
            SourceColor::Index(224),
            Role::RemovalBackground,
        ),
        (
            "diff.add.gutter.light.index",
            SourceColor::Index(157),
            Role::AdditionBackground,
        ),
        (
            "diff.remove.gutter.light.index",
            SourceColor::Index(217),
            Role::RemovalBackground,
        ),
    ] {
        result.push(Definition {
            token: token.into(),
            channel: Channel::Background,
            color,
            pair: None,
            target: Some(role.into()),
        });
    }
}

fn builtin(name: &str) -> Option<two_face::theme::EmbeddedThemeName> {
    use two_face::theme::EmbeddedThemeName as T;
    Some(match name {
        "ansi" => T::Ansi,
        "base16" => T::Base16,
        "base16-256" => T::Base16_256,
        "base16-eighties-dark" => T::Base16EightiesDark,
        "base16-mocha-dark" => T::Base16MochaDark,
        "base16-ocean-dark" => T::Base16OceanDark,
        "base16-ocean-light" => T::Base16OceanLight,
        "catppuccin-frappe" => T::CatppuccinFrappe,
        "catppuccin-latte" => T::CatppuccinLatte,
        "catppuccin-macchiato" => T::CatppuccinMacchiato,
        "catppuccin-mocha" => T::CatppuccinMocha,
        "coldark-cold" => T::ColdarkCold,
        "coldark-dark" => T::ColdarkDark,
        "dark-neon" => T::DarkNeon,
        "dracula" => T::Dracula,
        "github" => T::Github,
        "gruvbox-dark" => T::GruvboxDark,
        "gruvbox-light" => T::GruvboxLight,
        "inspired-github" => T::InspiredGithub,
        "1337" => T::Leet,
        "monokai-extended" => T::MonokaiExtended,
        "monokai-extended-bright" => T::MonokaiExtendedBright,
        "monokai-extended-light" => T::MonokaiExtendedLight,
        "monokai-extended-origin" => T::MonokaiExtendedOrigin,
        "nord" => T::Nord,
        "one-half-dark" => T::OneHalfDark,
        "one-half-light" => T::OneHalfLight,
        "solarized-dark" => T::SolarizedDark,
        "solarized-light" => T::SolarizedLight,
        "sublime-snazzy" => T::SublimeSnazzy,
        "two-dark" => T::TwoDark,
        "zenburn" => T::Zenburn,
        _ => return None,
    })
}
