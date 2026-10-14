use super::*;
use serde_json::Value;

/// The release whose palettes, loader and renderer the descriptor was extracted from.
macro_rules! verified_version {
    () => {
        "2.1.296"
    };
}

pub(super) const PROVIDER: Provider = Provider {
    version,
    verified: verified_version!(),
    resolve,
};

fn version(line: &str) -> Option<&str> {
    release(line.strip_suffix(" (Claude Code)")?)
}

pub(super) fn resolve(context: &LaunchContext, snapshot: &mut AgentThemeSnapshot) -> Result<()> {
    if [
        "CLAUDE_CODE_USE_COWORK_PLUGINS",
        "CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST",
    ]
    .iter()
    .any(|key| {
        context
            .environment
            .get(OsStr::new(key))
            .is_some_and(|v| !v.is_empty())
    }) {
        bail!("Claude host-managed or cowork configuration cannot be resolved by this descriptor");
    }
    let home = context.home()?;
    let root = context
        .environment
        .get(OsStr::new("CLAUDE_CONFIG_DIR"))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    let legacy = root.join(".config.json");
    let global = if legacy.exists() {
        legacy
    } else {
        context
            .environment
            .get(OsStr::new("CLAUDE_CONFIG_DIR"))
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or(home)
            .join(".claude.json")
    };
    let mut theme = "dark".to_owned();
    let apply = |value: &Value, theme: &mut String| -> Result<()> {
        if let Some(value) = value.get("theme") {
            *theme = value
                .as_str()
                .context("Claude theme must be a string")?
                .into();
        }
        Ok(())
    };
    if let Some(value) = read_json(&global)? {
        apply(&value, &mut theme)?;
    }
    let mut sources = vec!["user", "project", "local"];
    let mut settings = None;
    let mut args = context.arguments.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if matches!(arg.as_str(), "--safe-mode" | "--bare" | "--worktree" | "-w")
            || arg.starts_with("--worktree=")
        {
            bail!("Claude invocation changes configuration or directory after the managed launch; theme resolution is unavailable");
        }
        if arg == "--settings" {
            settings = Some(
                args.next()
                    .context("missing Claude settings argument")?
                    .as_str(),
            );
        }
        if let Some(value) = arg.strip_prefix("--settings=") {
            settings = Some(value);
        }
        if arg == "--setting-sources" {
            sources = args
                .next()
                .context("missing Claude setting sources")?
                .split(',')
                .collect();
        }
        if let Some(value) = arg.strip_prefix("--setting-sources=") {
            sources = value.split(',').collect();
        }
    }
    if sources.contains(&"local") {
        check_inherited_local_theme(context)?;
    }
    for (source, path) in [
        ("user", root.join("settings.json")),
        ("project", context.cwd.join(".claude/settings.json")),
        ("local", context.cwd.join(".claude/settings.local.json")),
    ] {
        if sources.contains(&source) {
            if let Some(value) = read_json(&path)? {
                apply(&value, &mut theme)?;
            }
        }
    }
    if let Some(settings) = settings {
        let value = if settings.trim_start().starts_with('{') {
            serde_json::from_str(settings).context("parse Claude launch settings")?
        } else {
            read_json(&context.cwd.join(settings))?.context("Claude launch settings are missing")?
        };
        apply(&value, &mut theme)?;
    }
    // The supported release also accepts organization settings, which outrank launch settings.
    let managed = if cfg!(target_os = "macos") {
        Path::new("/Library/Application Support/ClaudeCode/managed-settings.json")
    } else {
        Path::new("/etc/claude-code/managed-settings.json")
    };
    // Claude retains an empty remote-policy cache even when no policy is active.
    // Its presence alone must not disable the user's palette.
    let remote_policy = read_json(&root.join("remote-settings.json"))?;
    if remote_policy
        .as_ref()
        .is_some_and(|value| !value.as_object().is_some_and(|object| object.is_empty()))
        || managed.with_file_name("managed-settings.d").exists()
        || (cfg!(target_os = "macos")
            && Path::new("/Library/Managed Preferences/com.anthropic.claudecode.plist").exists())
    {
        bail!("Claude managed policy layers cannot be resolved by this descriptor");
    }
    if let Some(value) = read_json(managed)? {
        apply(&value, &mut theme)?;
    }
    snapshot.theme = theme.clone();
    snapshot.provenance = vec![concat!("claude-", verified_version!(), "/palettes-v2").into()];
    let mut presets: BTreeMap<String, BTreeMap<String, String>> = serde_json::from_str(
        include_str!(concat!("descriptors/claude-", verified_version!(), ".json")),
    )
    .context("read verified Claude palette descriptor")?;
    let (base, overrides) = if let Some(slug) = theme.strip_prefix("custom:") {
        if slug.is_empty()
            || Path::new(slug).components().count() != 1
            || slug.contains(['/', '\\', ':'])
        {
            bail!("plugin or invalid Claude custom theme cannot be resolved");
        }
        let path = root.join("themes").join(format!("{slug}.json"));
        let value = read_json(&path)?.context("selected Claude custom theme is missing")?;
        snapshot.provenance.push(path.display().to_string());
        (
            value
                .get("base")
                .and_then(Value::as_str)
                .unwrap_or("dark")
                .to_owned(),
            value.get("overrides").cloned(),
        )
    } else {
        (theme, None)
    };
    let mut tokens = presets
        .remove(if snapshot.theme == "auto" {
            "light"
        } else {
            &base
        })
        .context("Claude unknown base theme cannot be resolved")?;
    if let Some(overrides) = overrides.and_then(|v| v.as_object().cloned()) {
        for (token, value) in overrides {
            // Custom themes override UI diff tokens, never the renderer's fixed syntax palette.
            if token.starts_with("code.") {
                continue;
            }
            let Some(value) = value.as_str().filter(|value| color(value).is_some()) else {
                continue;
            };
            let replaced = match token.as_str() {
                "diffAdded" => Some("code.addLine"),
                "diffRemoved" => Some("code.deleteLine"),
                "diffAddedWord" => Some("code.addWord"),
                "diffRemovedWord" => Some("code.deleteWord"),
                _ => None,
            };
            if let Some(replaced) = replaced {
                tokens.remove(replaced);
                tokens.remove(&format!("{replaced}.index"));
            }
            if matches!(
                token.as_str(),
                "diffAdded"
                    | "diffRemoved"
                    | "diffAddedDimmed"
                    | "diffRemovedDimmed"
                    | "diffAddedWord"
                    | "diffRemovedWord"
            ) {
                tokens.insert(format!("code.{token}"), value.into());
            }
            if let std::collections::btree_map::Entry::Occupied(mut entry) = tokens.entry(token) {
                entry.insert(value.into());
            }
        }
    }
    add_code_indexes(&mut tokens);
    let mut tokens: Vec<_> = tokens.into_iter().collect();
    if snapshot.theme == "auto" {
        // Keep both variants of each token so either automatic palette can match.
        let mut dark = presets
            .remove("dark")
            .context("verified Claude dark palette is missing")?;
        add_code_indexes(&mut dark);
        tokens.extend(dark);
    }
    snapshot.definitions.clear();
    for (token, value) in tokens {
        let Some(color) = color(&value) else {
            continue;
        };
        let (channel, target) = token_role(&token);
        let extra_role = match token.as_str() {
            "diffAddedWord" => Some((Channel::Foreground, Role::Success)),
            "diffRemovedWord" => Some((Channel::Foreground, Role::Error)),
            "rate_limit_empty" => Some((Channel::Background, Role::Subtle)),
            _ => None,
        };
        if let Some((channel, role)) = extra_role {
            snapshot.definitions.push(Definition {
                token: token.clone(),
                channel,
                color,
                pair: None,
                target: Some(role.into()),
            });
        }
        snapshot.definitions.push(Definition {
            token,
            channel,
            color,
            pair: None,
            target,
        });
    }
    terminal_defaults(&mut snapshot.definitions);
    // Other highlighters and dynamic spinner gradients are outside the verified renderer.
    snapshot.availability = Availability::Partial(
        "UI and built-in code/diff palettes resolved; other highlighters and dynamic gradients pass through".into(),
    );
    Ok(())
}

fn check_inherited_local_theme(context: &LaunchContext) -> Result<()> {
    // 2.1.296 can canonicalize local settings to the repository root after checking
    // directory ownership, then merge legacy cwd settings underneath them. Do not
    // guess which file wins when this descriptor has not resolved those checks.
    let cwd = context
        .cwd
        .canonicalize()
        .context("resolve Claude settings directory")?;
    for root in cwd.ancestors() {
        if root
            .join(".git")
            .try_exists()
            .context("locate Claude repository settings")?
        {
            if root != cwd
                && read_json(&root.join(".claude/settings.local.json"))?
                    .is_some_and(|value| value.get("theme").is_some())
            {
                bail!("Claude inherited repository-local theme requires ownership-dependent configuration resolution");
            }
            break;
        }
    }
    Ok(())
}

fn token_role(token: &str) -> (Channel, Option<Target>) {
    use Role::*;
    if let Some(token) = token.strip_prefix("code.") {
        let token = token.strip_suffix(".index").unwrap_or(token);
        if token.starts_with("diff") {
            return token_role(token);
        }
        let (channel, role) = match token {
            "addLine" => (Channel::Background, AdditionBackground),
            "deleteLine" => (Channel::Background, RemovalBackground),
            "addWord" => (Channel::Background, AdditionWord),
            "deleteWord" => (Channel::Background, RemovalWord),
            "addDecoration" => (Channel::Foreground, Success),
            "deleteDecoration" => (Channel::Foreground, Error),
            "foreground" | "punctuation" | "subst" => (Channel::Foreground, Foreground),
            "comment" | "meta" => (Channel::Foreground, Muted),
            "string" | "regexp" => (Channel::Foreground, Success),
            "number" | "literal" | "symbol" => (Channel::Foreground, Warning),
            _ => (Channel::Foreground, Accent),
        };
        return (channel, Some(role.into()));
    }
    let token = token.strip_suffix("_FOR_SYSTEM_SPINNER").unwrap_or(token);
    let (base, shimmer) = token
        .strip_suffix("Shimmer")
        .map_or((token, false), |s| (s, true));
    let (channel, role) = match base {
        "text" => (Channel::Foreground, Some(Role::Foreground)),
        "background" => (Channel::Background, Some(Role::Background)),
        "inverseText" => (Channel::Foreground, Some(SelectionForeground)),
        "inactive" => (Channel::Foreground, Some(Muted)),
        "subtle" | "rate_limit_empty" => (Channel::Foreground, Some(Subtle)),
        "claude" | "permission" | "promptBorder" | "planMode" | "remember" | "suggestion"
        | "bashBorder" | "ide" | "fastMode" | "autoAccept" | "skill" | "effortUltra"
        | "briefLabelYou" | "briefLabelClaude" | "clawd_body" | "professionalBlue"
        | "chromeYellow" | "claudeBlue" | "rate_limit_fill" => (Channel::Foreground, Some(Accent)),
        "success" | "merged" => (Channel::Foreground, Some(Success)),
        "error" => (Channel::Foreground, Some(Error)),
        "warning" => (Channel::Foreground, Some(Warning)),
        "selectionBg" => (Channel::Background, Some(SelectionBackground)),
        "userMessageBackground"
        | "userMessageBackgroundHover"
        | "bashMessageBackgroundColor"
        | "memoryBackgroundColor"
        | "composerSidebarBackground" => (Channel::Background, Some(Surface)),
        "diffAdded" => (Channel::Background, Some(AdditionBackground)),
        "diffRemoved" => (Channel::Background, Some(RemovalBackground)),
        "diffAddedDimmed" => (Channel::Background, Some(AdditionDimmed)),
        "diffRemovedDimmed" => (Channel::Background, Some(RemovalDimmed)),
        "diffAddedWord" => (Channel::Background, Some(AdditionWord)),
        "diffRemovedWord" => (Channel::Background, Some(RemovalWord)),
        "clawd_background" => (Channel::Background, None),
        _ => (Channel::Foreground, None),
    };
    (channel, role.map(|role| Target { role, shimmer }))
}

fn add_code_indexes(tokens: &mut BTreeMap<String, String>) {
    // Dark addition fills have explicit indexed alternatives in the descriptor.
    let indexed = tokens
        .iter()
        .filter(|(token, _)| token.starts_with("code."))
        .filter_map(|(token, value)| match color(value)? {
            SourceColor::Rgb(r, g, b) if !tokens.contains_key(&format!("{token}.index")) => Some((
                format!("{token}.index"),
                format!("ansi256({})", code_color_index(r, g, b)),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    tokens.extend(indexed);
}

// 2.1.296's code renderer chooses the nearer xterm cube or grayscale color.
pub(super) fn code_color_index(r: u8, g: u8, b: u8) -> u8 {
    let cube = |v| match v {
        0..48 => 0,
        48..115 => 1,
        115..155 => 2,
        155..195 => 3,
        195..235 => 4,
        _ => 5,
    };
    let (ri, gi, bi) = (cube(r), cube(g), cube(b));
    let index = 16 + 36 * ri + 6 * gi + bi;
    let average = (f64::from(r) + f64::from(g) + f64::from(b)) / 3.0;
    let average = average.round();
    if average < 5.0 {
        return 16;
    }
    if average > 244.0 && ri == gi && gi == bi {
        return index;
    }
    let gray = ((average - 8.0) / 10.0).round().clamp(0.0, 23.0) as u8;
    let levels = [0, 95, 135, 175, 215, 255];
    let distance = |rgb: [i32; 3]| {
        [r, g, b]
            .into_iter()
            .zip(rgb)
            .map(|(v, level)| (i32::from(v) - level).pow(2))
            .sum::<i32>()
    };
    if distance([8 + i32::from(gray) * 10; 3])
        < distance([
            levels[ri as usize],
            levels[gi as usize],
            levels[bi as usize],
        ])
    {
        232 + gray
    } else {
        index
    }
}
