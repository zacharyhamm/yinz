//! Native builds embed all Scheme plugin sources.
use crate::flags;
use anyhow::Context;
use xshell::{cmd, Shell};

pub fn build(sh: &Shell, flags: flags::Build) -> anyhow::Result<()> {
    let _pd = sh.push_dir(crate::project_root());
    anyhow::ensure!(
        !(flags.release && flags.profile.is_some()),
        "cannot use both --release and --profile"
    );
    run_proto_codegen(sh, false);
    let cargo = crate::cargo()?;
    let profile = profile_of(&flags);
    crate::status(">> Building 'yinz'");
    cmd!(sh, "{cargo} build --profile {profile} --bin yinz")
        .args(&flags.args)
        .run()
        .context("failed to build 'yinz'")
}

pub fn profile_of(flags: &flags::Build) -> &str {
    match flags.profile.as_deref() {
        Some(profile) => profile,
        None if flags.release => "release",
        None => "dev",
    }
}

pub fn profile_dir(profile: &str) -> &str {
    match profile {
        "dev" | "test" => "debug",
        "release" | "bench" => "release",
        other => other,
    }
}

pub fn proto(sh: &Shell) -> anyhow::Result<()> {
    let msg = ">> Generating protobuffer code";
    crate::status(msg);
    println!("{}", msg);

    run_proto_codegen(sh, true);
    Ok(())
}

pub(crate) fn run_proto_codegen(sh: &Shell, force: bool) {
    let _pd = sh.push_dir(crate::project_root());

    let specs: &[(&str, &str, &str)] = &[
        (
            "yinz-plugin-api/assets",
            "yinz-plugin-api/src",
            "generated_plugin_api.rs",
        ),
        (
            "yinz-ipc/assets/prost_ipc",
            "yinz-ipc/src/client_server_contract",
            "generated_client_server_api.rs",
        ),
        (
            "yinz-host-protocol/assets/prost_nested_session",
            "yinz-host-protocol/src/nested_session_contract",
            "generated_nested_session_api.rs",
        ),
    ];

    for (out_subdir, src_subdir, include_file) in specs {
        let out_dir = sh.current_dir().join(out_subdir);
        let src_dir = sh.current_dir().join(src_subdir);
        std::fs::create_dir_all(&out_dir).unwrap();

        let last_generated = out_dir
            .join(include_file)
            .metadata()
            .and_then(|m| m.modified());
        let mut proto_files = vec![];
        let mut needs_regeneration = force;

        for entry in std::fs::read_dir(&src_dir).unwrap() {
            let entry_path = entry.unwrap().path();
            if entry_path.is_file()
                && entry_path
                    .extension()
                    .map(|e| e == "proto")
                    .unwrap_or(false)
            {
                let modified = entry_path.metadata().and_then(|m| m.modified());
                needs_regeneration |= match (&last_generated, modified) {
                    (Ok(last_generated), Ok(modified)) => modified > *last_generated,
                    // Couldn't read some metadata, assume needs update
                    _ => true,
                };
                proto_files.push(entry_path.display().to_string());
            }
        }
        proto_files.sort();

        if needs_regeneration {
            let mut prost = prost_build::Config::new();
            prost.out_dir(&out_dir);
            prost.include_file(include_file);
            prost.compile_protos(&proto_files, &[src_dir]).unwrap();
        }
    }
}

#[cfg(test)]
#[path = "unit/build.rs"]
mod tests;
