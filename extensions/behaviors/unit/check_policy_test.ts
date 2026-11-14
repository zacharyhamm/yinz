import {
  assertCheckPolicy,
  CARGO_ALIASES,
  CHECK_POLICY_SUMMARY,
  checkPlanPolicy,
  checkPolicyViolations,
} from "../_lib/check_policy.ts";

function assert(value: unknown, message = "Assertion failed"): asserts value {
  if (!value) throw new Error(message);
}
function rejected(argv: string[], text: string) {
  const violations = checkPolicyViolations(argv);
  assert(
    violations.some((v) => v.includes(text)),
    `${JSON.stringify(argv)} should be rejected with "${text}": ${
      JSON.stringify(violations)
    }`,
  );
}

Deno.test("check policy accepts focused repository checks", () => {
  for (
    const argv of [
      ["cargo", "x", "test", "-p", "yinz-server"],
      ["cargo", "x", "test", "some_test_name"],
      ["cargo", "x", "test", "-p", "yinz-screen", "-E", "test(/^screen::/)"],
      ["cargo", "x", "test", "--package=yinz-screen"],
      ["cargo", "x", "test", "-pyinz-screen"],
      ["cargo", "x", "test", "--workspace", "-E", "test(resize)"],
      ["cargo", "x", "integration-test", "-E", "test(resize)", "-j", "16"],
      ["cargo", "x", "integration-test", "-j16", "resize"],
      ["cargo", "x", "integration-test", "--jobs=4", "-E", "test(x)"],
      ["cargo", "x", "integration-test", "--serial", "-E", "test(x)"],
      ["cargo", "test", "-p", "yinz-tab"],
      ["cargo", "+stable", "test", "-p", "yinz-tab"],
      ["cargo", "--locked", "test", "-p", "yinz-tab"],
      ["cargo", "nextest", "run", "-p", "x"],
      ["cargo", "x", "lint"],
      ["cargo", "x", "format", "--check"],
      ["cargo", "x", "build"],
      ["cargo", "check", "-p", "yinz-tab"],
      ["deno", "test", "command.ts"],
      ["grep", "-c", "a|b", "m.txt"],
      ["grep", "-E", "x;y", "m.txt"],
      ["/usr/bin/true"],
      [
        "python3",
        "-m",
        "unittest",
        "discover",
        "-s",
        "scripts/issue_lifecycle/unit",
        "-p",
        "test_git_ops.py",
      ],
      ["env", "RUST_LOG=debug", "cargo", "x", "test", "-p", "a"],
      ["env", "-u", "SWAMP_REPO_DIR", "deno", "task", "check"],
      ["cargo", "x", "test", "--", "-p", "a", "--", "--exact", "name"],
      ["timeout", "900", "cargo", "x", "test", "-p", "a"],
      ["timeout", "-k", "10", "900", "cargo", "x", "lint"],
      ["nice", "-n", "5", "cargo", "x", "format", "--check"],
      [
        "env",
        "-C",
        ".",
        "cargo",
        "x",
        "integration-test",
        "-E",
        "test(x)",
        "-j",
        "4",
      ],
      ["grep", "-q", "cargo", "Cargo.toml"],
      ["grep", "-q", "cargo", "f"],
      [
        "cargo",
        "x",
        "test",
        "-p",
        "xtask",
        "-E",
        "test(=metadata::tests::current_workspace_respects_dependency_boundaries)",
        "--no-tests=warn",
      ],
      ["cargo", "x", "test", "-p", "a", "--no-tests", "warn"],
      ["cargo", "x", "test", "--", "-p", "a", "--", "--skip", "slow"],
      ["cargo", "x", "test", "-p", "a", "--skip", "slow"],
      ["cargo", "x", "test", "--", "-p", "a", "--", "--skip", "slow", "fast"],
      ["env", "-a", "name", "cargo", "x", "test", "-p", "a"],
      ["env", "-", "PATH=/usr/bin", "cargo", "x", "lint"],
      ["cargo", "x", "integration-test", "--serial", "--", "-E", "test(x)"],
      ["cargo", "x", "test", "-E", "not test(slow)"],
      ["cargo", "x", "test", "--", "--skip=slow", "fast"],
      ["cargo", "run", "-p", "xtask", "--", "test", "-p", "a"],
      ["cargo", "run", "--package=xtask", "--", "lint"],
      ["cargo", "r", "-pxtask", "--", "format", "--check"],
      ["cargo", "xtask", "integration-test", "-E", "test(x)", "-j", "4"],
      ["cargo-nextest", "nextest", "run", "-p", "a"],
      ["cargo", "nextest", "--color", "never", "run", "-p", "a"],
      ["env", "-iv", "RUST_LOG=1", "cargo", "x", "test", "-p", "a"],
      // A named test target narrows a run like a package does.
      ["cargo", "test", "--test", "foo"],
      ["cargo", "x", "test", "--bench", "b"],
      ["cargo", "nextest", "run", "--bin=yinz"],
      ["cargo", "x", "test", "-p=yinz-tab"],
      // xtask passes everything after its `--` to nextest as top-level
      // options, so `-p a` there names a package and `-j` bounds workers.
      ["cargo", "x", "test", "--", "-p", "a"],
      ["cargo", "x", "integration-test", "--", "-E", "test(x)", "-j", "16"],
      ["cargo", "x", "integration-test", "--serial", "--", "-E", "test(x)"],
      ["cargo", "x", "test", "--", "--", "name"],
      ["cargo", "x", "test", "-p", "xtask"],
      ["test", "-x", "xtask"],
      ["cargo", "--config", "net.retry=2", "test", "-p", "a"],
      ["env", "-C", "xtask", "cargo", "test", "-p", "xtask"],
      ["env", "-C", "xtask/..", "cargo", "run", "--", "--help"],
      ["cargo", "run", "-m", "Cargo.toml", "--", "--help"],
      ["cargo", "run", "-p", "yinz@0.1.0", "--", "ci"],
    ]
  ) {
    const violations = checkPolicyViolations(argv);
    assert(
      violations.length === 0,
      `${JSON.stringify(argv)}: ${JSON.stringify(violations)}`,
    );
  }
});

Deno.test("check policy rejects unfiltered suites and mutating xtask commands", () => {
  for (
    const [argv, text] of [
      [["cargo", "x", "test"], "cargo x test requires a package"],
      [["cargo", "x", "test", "--workspace"], "cargo x test requires"],
      [["cargo", "x", "test", "-p", "a", "--workspace"], "cargo x test"],
      [["cargo", "x", "test", "-j", "16"], "cargo x test requires"],
      [["cargo", "x", "test", "--run-ignored", "all"], "cargo x test"],
      [["cargo", "x", "test", "--", "--exact"], "cargo x test requires"],
      [
        ["cargo", "x", "integration-test", "-j", "16"],
        "requires a test selection",
      ],
      [
        ["cargo", "x", "integration-test", "-E", "test(x)"],
        "requires -j N (or --serial)",
      ],
      [["cargo", "x", "ci", "check"], "merge gate"],
      [["cargo", "test"], "cargo test requires"],
      [["cargo", "t", "--workspace"], "cargo test requires"],
      [["cargo", "test", "--workspace"], "cargo test requires"],
      [["cargo", "nextest", "run"], "cargo nextest run requires"],
      [["cargo", "nextest", "run", "--workspace"], "cargo nextest run"],
      [["cargo", "x", "format"], "format --check"],
      [["cargo", "x", "lint", "--fix"], "drop --fix"],
      [["cargo", "x", "run"], "cargo x run is not a focused"],
      [["cargo", "x", "bench"], "cargo x bench is not a focused"],
      [["cargo", "x", "install", "/tmp/x"], "cargo x install is not"],
      [["cargo", "x"], "cargo x is not a focused"],
      [["cargo", "xtask", "remote-test"], "not a focused"],
    ] as const
  ) rejected([...argv], text);
});

Deno.test("check policy rejects nextest value flags mistaken for test names", () => {
  for (
    const argv of [
      ["cargo", "x", "test", "--no-tests", "warn"],
      ["cargo", "nextest", "run", "--no-tests", "pass"],
      ["cargo", "x", "test", "--build-jobs", "8"],
      ["cargo", "x", "test", "--config-file", "x.toml"],
      ["cargo", "x", "test", "--tool-config-file", "t:/x.toml"],
      ["cargo", "x", "test", "--message-format-version", "0.1"],
      // A target selector whose glob names every target is not a selection.
      ["cargo", "x", "test", "--bench", "*"],
      ["cargo", "test", "--test", "*"],
      ["cargo", "x", "test", "--lib"],
      ["cargo", "x", "test", "--tests"],
      ["cargo", "x", "test", "-p", "*"],
      ["cargo", "test", "-m", "Cargo.toml"],
      ["cargo", "nextest", "run", "--manifest-path", "Cargo.toml"],
      ["cargo", "x", "test", "--archive-file", "a.tar.zst"],
      ["cargo", "x", "test", "--workspace-remap", "/w"],
      ["cargo", "x", "test", "--stress-count", "3"],
      ["cargo", "x", "test", "-R", "latest"],
      ["cargo", "test", "--lockfile-path", "Cargo.lock"],
      // --skip excludes tests; its pattern is not a selection.
      ["cargo", "x", "test", "--", "--skip", "slow"],
      ["cargo", "x", "test", "--", "--exact", "--skip", "slow"],
      ["cargo", "test", "--", "--skip", "a", "--skip", "b"],
      ["cargo", "x", "test", "--", "--skip=slow"],
      ["cargo", "x", "test", "--", "--format", "json"],
    ]
  ) rejected(argv, "requires a package");
  rejected(
    ["cargo", "x", "integration-test", "-j", "16", "--no-tests", "pass"],
    "requires a test selection",
  );
  rejected(
    ["cargo", "x", "integration-test", "-j", "4", "--", "--skip", "slow"],
    "requires a test selection",
  );
});

Deno.test("check policy reads xtask's forwarded arguments as nextest options", () => {
  // xtask drops its first `--` and appends the rest to `cargo nextest run`,
  // so the values of nextest options there are not test names.
  for (
    const argv of [
      ["cargo", "x", "test", "--", "--no-tests", "warn"],
      ["cargo", "x", "test", "--", "--run-ignored", "all"],
      ["cargo", "x", "test", "--", "-P", "unit"],
      ["cargo", "x", "test", "--", "--retries", "2"],
      ["cargo", "x", "test", "--", "--", "--skip", "slow"],
      ["cargo", "x", "test", "--", "--", ""],
    ]
  ) rejected(argv, "cargo x test requires a package");
  rejected(
    [
      "cargo",
      "x",
      "integration-test",
      "--serial",
      "--",
      "--run-ignored",
      "all",
    ],
    "requires a test selection",
  );
  // A --serial after the forwarding point reaches nextest, not xtask.
  rejected(
    ["cargo", "x", "integration-test", "-E", "test(x)", "--serial"],
    "requires -j N (or --serial)",
  );
  rejected(
    ["cargo", "x", "integration-test", "-E", "test(x)", "--", "--serial"],
    "requires -j N (or --serial)",
  );
});

Deno.test("check policy rejects selections that match every test", () => {
  for (
    const argv of [
      ["cargo", "x", "test", ""],
      ["cargo", "x", "test", "-E", "all()"],
      ["cargo", "x", "test", "-E", " true( ) "],
      ["cargo", "x", "test", "--filterset=all()"],
      ["cargo", "x", "test", "-p", "a", "-p", "*"],
      ["cargo", "nextest", "run", "-p", "*", "-p", "a"],
      ["cargo", "test", "-p", "a", "--package=*"],
    ]
  ) rejected(argv, "requires a package");
});

Deno.test("check policy applies the xtask rules to the xtask binary", () => {
  for (
    const [argv, text] of [
      [["target/debug/xtask", "ci", "check"], "merge gate"],
      [["./target/debug/xtask", "test"], "cargo x test requires"],
      [["setsid", "target/release/xtask", "ci"], "merge gate"],
      [["timeout", "60", "xtask", "ci"], "merge gate"],
      [["env", "-S", "target/debug/xtask ci"], "merge gate"],
      [["xtask", "integration-test", "-E", "test(x)"], "requires -j N"],
    ] as const
  ) rejected([...argv], text);
  const focused = checkPolicyViolations([
    "target/debug/xtask",
    "test",
    "-p",
    "a",
  ]);
  assert(focused.length === 0, JSON.stringify(focused));
});

Deno.test("check policy resolves xtask through cargo run, aliases and cargo-nextest", () => {
  for (
    const [argv, text] of [
      [
        ["cargo", "run", "--package", "xtask", "--", "ci", "check"],
        "merge gate",
      ],
      [["cargo", "run", "-p", "xtask", "--", "test"], "cargo x test requires"],
      [["cargo", "run", "--package=xtask", "--", "ci"], "merge gate"],
      [["cargo", "run", "-pxtask", "--", "ci"], "merge gate"],
      [["cargo", "r", "-p", "xtask", "--", "ci"], "merge gate"],
      [["cargo", "run", "--bin", "xtask", "--", "ci"], "merge gate"],
      [
        ["cargo", "run", "--manifest-path", "xtask/Cargo.toml", "--", "ci"],
        "merge gate",
      ],
      [["cargo", "--locked", "run", "-p", "xtask", "--", "ci"], "merge gate"],
      [["cargo", "run", "-p", "xtask"], "cargo x is not a focused"],
      [
        ["cargo", "run", "-p", "xtask", "--", "integration-test"],
        "requires a test selection",
      ],
      [["cargo", "q"], "cargo x run is not a focused"],
      [["cargo", "q"], "a check must terminate"],
      [["cargo", "x", "run"], "a check must terminate"],
      [["cargo", "x", "run", "--quick-run"], "a check must terminate"],
      [["cargo", "make"], "cargo x deprecated is not a focused"],
      [["cargo", "+stable", "q"], "cargo x run"],
      [["cargo-nextest", "nextest", "run"], "cargo nextest run requires"],
      [["/x/bin/cargo-nextest", "run"], "cargo nextest run requires"],
      [["cargo", "nextest", "--no-pager", "run"], "cargo nextest run requires"],
      [["timeout", "60", "cargo-nextest", "nextest", "r"], "cargo nextest run"],
    ] as const
  ) rejected([...argv], text);
  // Other cargo run invocations are not xtask.
  for (
    const argv of [
      ["cargo", "run", "-p", "a", "--", "ci"],
      ["cargo", "run", "--bin", "yinz", "--", "ci"],
      ["cargo", "run", "--", "ci"],
      ["cargo", "run", "--manifest-path", "xtask/../Cargo.toml", "--", "ci"],
    ]
  ) {
    const violations = checkPolicyViolations(argv);
    assert(violations.length === 0, JSON.stringify([argv, violations]));
  }
  const other = checkPolicyViolations(["cargo", "run", "-p", "a", "--", "ci"]);
  assert(other.length === 0, JSON.stringify(other));
});

Deno.test("check policy finds xtask behind every cargo run spelling", () => {
  for (
    const argv of [
      ["cargo", "run", "-m", "xtask/Cargo.toml", "--", "ci", "check"],
      ["cargo", "run", "-mxtask/Cargo.toml", "--", "ci", "check"],
      ["cargo", "run", "-m=xtask/Cargo.toml", "--", "ci"],
      ["cargo", "run", "--manifest-path=./xtask/./Cargo.toml", "--", "ci"],
      ["cargo", "run", "-m", "xtask/src/../Cargo.toml", "--", "ci"],
      ["cargo", "run", "-m", "/abs/repo/xtask/Cargo.toml", "--", "ci"],
      ["cargo", "run", "-p=xtask", "--", "ci", "check"],
      ["cargo", "run", "--package", "xtask@0.1.0", "--", "ci"],
      ["cargo", "run", "-p", "xtask:0.1.0", "--", "ci"],
      ["cargo", "run", "-p", "path+file:///repo/xtask", "--", "ci"],
      ["cargo", "run", "-p", "path+file:///repo/xtask#0.1.0", "--", "ci"],
      ["cargo", "run", "-p", "path+file:///repo#xtask@0.1.0", "--", "ci"],
      ["cargo", "run", "-p", "xt*", "--", "ci"],
      ["cargo", "run", "--bin=xtask", "--", "ci"],
      ["cargo", "--manifest-path", "xtask/Cargo.toml", "run", "--", "ci"],
      // cargo run without a package runs the current directory's package.
      ["env", "-C", "xtask", "cargo", "run", "--", "ci", "check"],
      ["env", "--chdir=./xtask/src", "cargo", "run", "--", "ci"],
      ["env", "-C", "xtask", "cargo", "run", "-m", "Cargo.toml", "--", "ci"],
      [
        "cargo",
        "+nightly",
        "-Zunstable-options",
        "-C",
        "xtask",
        "run",
        "--",
        "ci",
      ],
    ]
  ) rejected(argv, "merge gate");
});

Deno.test("check policy rejects cargo aliases defined by the check", () => {
  for (
    const argv of [
      ["cargo", "--config", 'alias.zz="x ci check"', "zz"],
      ["cargo", "--config=alias.zz=['x', 'ci']", "zz"],
      ["cargo", "--config", "alias = { zz = 'x ci' }", "zz"],
    ]
  ) rejected(argv, "never define cargo aliases through --config");
  for (
    const argv of [
      ["env", "CARGO_ALIAS_ZZ=x ci check", "cargo", "zz"],
      ["env", "-S", "CARGO_ALIAS_ZZ=x cargo zz"],
      ["CARGO_ALIAS_ZZ=x", "cargo", "zz"],
    ]
  ) rejected(argv, "never define cargo aliases in a check");
  rejected(
    ["cargo", "--config", "extra.toml", "test", "-p", "a"],
    "--config files",
  );
});

Deno.test("check policy evaluates every cargo in argv, not only the first", () => {
  for (
    const argv of [
      ["xargs", "-a", "cargo", "cargo", "x", "ci", "check"],
      ["setsid", "env", "-S", "cargo x ci check"],
      ["xargs", "nice", "cargo", "x", "ci"],
    ]
  ) rejected(argv, "merge gate");
});

Deno.test("check policy splits env -S text as GNU env does", () => {
  for (
    const argv of [
      ["env", "-vS", "cargo x ci check"],
      ["env", "-ivS", "cargo x ci check"],
      ["env", "-v", "-S", "cargo x ci check"],
      ["env", "-S", "'cargo' x ci check"],
      ["env", "-S", '"cargo" "x" ci check'],
      ["env", "-S", "cargo\\_x\\_ci\\_check"],
      ["env", "-S", "car\\go x ci"],
      ["env", "--split=cargo x ci"],
      ["env", "--sp", "cargo x ci"],
      ["env", "-S", "-S cargo x ci"],
    ]
  ) rejected(argv, "merge gate");
  for (
    const argv of [
      ["env", "-S", "A=1\\_CARGO_TARGET_DIR=/t cargo test -p a"],
      ["env", "-S", 'A=1 "CARGO_TARGET_DIR"=/t cargo test -p a'],
      ["env", "-S", "'CARGO_TARGET_DIR=/t' cargo test -p a"],
      ["env", "-vSA=1\\_CARGO_TARGET_DIR=/t cargo test -p a"],
      ["env", "--un", "CARGO_TARGET_DIR", "cargo", "test", "-p", "a"],
      ["env", "-iuCARGO_TARGET_DIR", "cargo", "test", "-p", "a"],
    ]
  ) rejected(argv, "never set CARGO_TARGET_DIR");
  // Quoted and commented -S text that stays focused is accepted.
  for (
    const argv of [
      ["env", "-S", "RUST_LOG='a b' cargo x test -p a # trailing comment"],
      ["env", "-S", 'cargo x test -E "test(a b)"'],
      ["env", "-S", "cargo x lint \\c x ci"],
    ]
  ) {
    const violations = checkPolicyViolations(argv);
    assert(violations.length === 0, JSON.stringify([argv, violations]));
  }
});

Deno.test("cargo alias table mirrors .cargo/config.toml", async () => {
  const text = await Deno.readTextFile(
    new URL("../../../.cargo/config.toml", import.meta.url),
  );
  const aliases: Record<string, string> = {};
  let section = "";
  for (const raw of text.split("\n")) {
    const line = raw.replace(/#.*$/, "").trim();
    if (!line) continue;
    const header = line.match(/^\[(.+)\]$/);
    if (header) {
      section = header[1].trim();
      continue;
    }
    if (section !== "alias") continue;
    const entry = line.match(/^([A-Za-z0-9_-]+)\s*=\s*"([^"]*)"$/);
    assert(
      entry,
      `Unsupported alias form in .cargo/config.toml: ${line}; ` +
        "extend CARGO_ALIASES and this parser",
    );
    aliases[entry[1]] = entry[2];
  }
  assert(Object.keys(aliases).length > 0, "No [alias] entries were found");
  assert(
    JSON.stringify(Object.entries(aliases).sort()) ===
      JSON.stringify(Object.entries(CARGO_ALIASES).sort()),
    `CARGO_ALIASES ${JSON.stringify(CARGO_ALIASES)} must match ` +
      `.cargo/config.toml ${JSON.stringify(aliases)}; classify any new alias`,
  );
});

Deno.test("check policy rejects libtest flags", () => {
  for (
    const argv of [
      ["cargo", "x", "test", "-p", "a", "--", "--nocapture"],
      ["cargo", "test", "-p", "a", "--", "--test-threads=1"],
      ["cargo", "x", "test", "-p", "a", "--test-threads", "2"],
    ]
  ) rejected(argv, "libtest flag");
});

Deno.test("check policy rejects shell constructs", () => {
  for (
    const [argv, text] of [
      [["sh", "-c", "cargo x test -p a"], "shell -c wrapper"],
      [["/bin/bash", "-lc", "cargo x test -p a"], "shell -c wrapper"],
      [["env", "FOO=1", "bash", "-c", "true"], "shell -c wrapper"],
      [["timeout", "60", "sh", "-c", "true"], "shell -c wrapper"],
      [
        ["cargo", "x", "test", "-p", "a", "&&", "cargo", "x", "lint"],
        "shell operator",
      ],
      [["deno", "test", ">", "out.txt"], "shell operator"],
      [["cargo x test -p a"], "split the command"],
      [["cargo", "x", "test", "-p a && cargo x lint"], "embedded shell"],
      [["ls", "$HOME/x"], "not expanded without a shell"],
      [["ls", "${HOME}"], "not expanded without a shell"],
      [["ls", "~/x"], "not expanded without a shell"],
      [["ls", "~"], "not expanded without a shell"],
      [["FOO=1", "cargo", "x", "test", "-p", "a"], "requires env"],
    ] as const
  ) rejected([...argv], text);
  // Unfiltered cargo commands behind an assignment are still evaluated.
  rejected(["FOO=1", "cargo", "test"], "cargo test requires");
});

Deno.test("check policy applies cargo rules behind transparent wrappers", () => {
  for (
    const [argv, text] of [
      [["timeout", "900", "cargo", "x", "ci", "check"], "merge gate"],
      [["timeout", "--signal=KILL", "1h", "cargo", "x", "ci"], "merge gate"],
      [["timeout", "-s", "KILL", "60", "cargo", "test"], "cargo test requires"],
      [["nice", "cargo", "x", "integration-test"], "requires a test selection"],
      [["nice", "-n10", "cargo", "x", "integration-test"], "requires -j N"],
      [["ionice", "-c", "3", "cargo", "nextest", "run"], "cargo nextest run"],
      [["time", "-p", "cargo", "x", "test"], "cargo x test requires"],
      [["/usr/bin/time", "-o", "t.txt", "cargo", "x", "ci"], "merge gate"],
      [["stdbuf", "-oL", "cargo", "x", "lint", "--fix"], "drop --fix"],
      [["nohup", "cargo", "x", "format"], "format --check"],
      [["env", "-C", ".", "cargo", "x", "ci", "check"], "merge gate"],
      [["env", "--chdir=.", "cargo", "x", "integration-test"], "selection"],
      [["env", "-S", "cargo x ci check"], "merge gate"],
      [["env", "--split-string=FOO=1 cargo test"], "cargo test requires"],
      [
        ["timeout", "60", "env", "RUST_LOG=1", "cargo", "x", "ci"],
        "merge gate",
      ],
      [["nice", "timeout", "60", "cargo", "x", "ci"], "merge gate"],
      // An unknown runner cannot hide a later cargo invocation either.
      [["xargs", "-a", "args.txt", "cargo", "x", "ci", "check"], "merge gate"],
      [["env", "--unknown", "cargo", "x", "ci"], "merge gate"],
      [
        [
          "timeout",
          "60",
          "env",
          "CARGO_TARGET_DIR=/t",
          "cargo",
          "test",
          "-p",
          "a",
        ],
        "never set CARGO_TARGET_DIR",
      ],
      [
        ["env", "-C", ".", "CARGO_TARGET_DIR=/t", "cargo", "test", "-p", "a"],
        "never set CARGO_TARGET_DIR",
      ],
    ] as const
  ) rejected([...argv], text);
});

Deno.test("check policy rejects target-dir changes", () => {
  for (
    const argv of [
      ["env", "CARGO_TARGET_DIR=/tmp/t", "cargo", "x", "test", "-p", "a"],
      ["env", "CARGO_BUILD_TARGET_DIR=/tmp/t", "deno", "test"],
      ["CARGO_TARGET_DIR=/tmp/t", "cargo", "x", "test", "-p", "a"],
      ["env", "-u", "CARGO_TARGET_DIR", "cargo", "test", "-p", "a"],
      // Found wherever it appears, independent of unwrapping.
      [
        "env",
        "--ignore-signal",
        "CARGO_TARGET_DIR=/t",
        "cargo",
        "test",
        "-p",
        "a",
      ],
      ["env", "-iv", "CARGO_TARGET_DIR=/t", "cargo", "test", "-p", "a"],
      ["env", "--bogus", "CARGO_TARGET_DIR=/t", "cargo", "test", "-p", "a"],
      ["setsid", "env", "CARGO_TARGET_DIR=/t", "cargo", "x", "test", "-p", "a"],
      [
        "rustup",
        "run",
        "stable",
        "env",
        "CARGO_TARGET_DIR=/t",
        "cargo",
        "x",
        "test",
        "-p",
        "a",
      ],
      ["xargs", "env", "CARGO_TARGET_DIR=/t", "cargo", "test", "-p", "a"],
      ["env", "-S", "CARGO_TARGET_DIR=/t cargo test -p a"],
      ["env", "-SCARGO_TARGET_DIR=/t cargo test -p a"],
      ["env", "-iSCARGO_TARGET_DIR=/t cargo test -p a"],
      ["env", "--split-string=CARGO_TARGET_DIR=/t cargo test -p a"],
      ["setsid", "env", "-S", "FOO=1 CARGO_BUILD_TARGET_DIR=/t", "cargo"],
      ["FOO=1", "CARGO_TARGET_DIR=/t", "cargo", "test", "-p", "a"],
    ]
  ) rejected(argv, "never set CARGO_TARGET_DIR");
  // Similar names are not the target-dir variables.
  for (
    const argv of [
      ["env", "MY_CARGO_TARGET_DIR=/t", "cargo", "test", "-p", "a"],
      ["env", "XCARGO_TARGET_DIR=/t", "deno", "test"],
    ]
  ) {
    const violations = checkPolicyViolations(argv);
    assert(violations.length === 0, JSON.stringify(violations));
  }
  for (
    const argv of [
      ["cargo", "test", "-p", "a", "--target-dir", "/tmp/t"],
      ["cargo", "test", "-p", "a", "--target-dir=/tmp/t"],
    ]
  ) rejected(argv, "--target-dir");
  for (
    const argv of [
      ["cargo", "--config", "build.target-dir='/tmp/t'", "test", "-p", "a"],
      ["cargo", "test", "-p", "a", "--config=build.target-dir='/tmp/t'"],
    ]
  ) rejected(argv, "build.target-dir");
  // A shared build directory breaks worktree isolation like a target-dir.
  for (
    const argv of [
      ["env", "CARGO_BUILD_BUILD_DIR=/tmp/s", "cargo", "x", "test", "-p", "a"],
      ["env", "-S", "CARGO_BUILD_BUILD_DIR=/t cargo test -p a"],
      ["setsid", "CARGO_BUILD_BUILD_DIR=/t", "cargo", "test", "-p", "a"],
    ]
  ) rejected(argv, "never set CARGO_TARGET_DIR or move the target or build");
  rejected(
    [
      "cargo",
      "--config",
      'build.build-dir="/tmp/shared"',
      "x",
      "test",
      "-p",
      "a",
    ],
    "build.build-dir",
  );
});

Deno.test("violations are aggregated with check index and id", () => {
  const checks = [
    { id: "ok", command: ["cargo", "x", "lint"], covers: ["a"] },
    { id: "suite", command: ["cargo", "x", "test"], covers: ["a"] },
    { id: "shell", command: ["sh", "-c", "true"], covers: ["a"] },
  ];
  const violations = checkPlanPolicy(checks);
  assert(violations.length === 2, JSON.stringify(violations));
  assert(violations[0].startsWith('checks[1] (id "suite"): '), violations[0]);
  assert(violations[1].startsWith('checks[2] (id "shell"): '), violations[1]);
  try {
    assertCheckPolicy(checks);
  } catch (error) {
    const message = (error as Error).message;
    assert(
      message.startsWith("Plan checks violate repository test policy:\n- "),
      message,
    );
    assert(message.includes('checks[1] (id "suite")'), message);
    assert(message.includes('checks[2] (id "shell")'), message);
    assertCheckPolicy([checks[0]]);
    assert(CHECK_POLICY_SUMMARY.startsWith("Check policy"), "Summary");
    return;
  }
  throw new Error("Policy violations were accepted");
});

Deno.test("check policy rejects flags xtask refuses before an explicit --", () => {
  for (
    const argv of [
      ["cargo", "x", "test", "-p", "a", "--", "--exact", "name"],
      ["cargo", "x", "test", "-p", "a", "--", "--skip", "slow"],
      ["cargo", "x", "integration-test", "-j", "4", "--", "-E", "test(x)"],
    ]
  ) rejected(argv, "before an explicit --");
});

Deno.test("check policy rejects package globs and filtersets matching all", () => {
  for (
    const argv of [
      ["cargo", "x", "test", "-p", "yinz-*"],
      ["cargo", "test", "--package=yinz-?ab"],
      ["cargo", "x", "test", "-E", "not none()"],
      ["cargo", "nextest", "run", "-E", "!none()"],
    ]
  ) rejected(argv, "requires a package");
});

Deno.test("check policy fails closed on env options it does not parse", () => {
  // GNU env's -a/--argv0 takes a value; the -S string after it is split.
  for (
    const argv of [
      ["env", "-a", "foo", "-S", "cargo x ci"],
      ["env", "--argv0", "foo", "-S", "cargo x ci"],
      ["env", "--argv0=foo", "-S", "cargo x ci"],
      ["env", "-ia", "foo", "-S", "cargo x ci"],
      ["env", "-afoo", "-S", "cargo x ci"],
      ["env", "--arg", "foo", "-S", "cargo x ci"],
      // An unknown option is rejected, and later -S strings are still split.
      ["env", "-P", "/bin", "-S", "cargo x ci"],
      ["env", "--bogus", "-ivS", "cargo x ci"],
      ["setsid", "env", "-P", "/bin", "--split-string=cargo x ci"],
    ]
  ) rejected(argv, "merge gate");
  rejected(["env", "--argv0", "foo", "-S", "cargo x test"], "cargo x test");
  for (
    const argv of [
      ["env", "-a", "foo", "-S", "A=1\\_CARGO_TARGET_DIR=/x cargo x test -p a"],
      [
        "env",
        "--argv0=foo",
        "-S",
        "A=1\\_CARGO_TARGET_DIR=/x cargo x test -p a",
      ],
      [
        "env",
        "--argv0",
        "foo",
        "-S",
        "A=1\\_CARGO_TARGET_DIR=/x cargo x test -p a",
      ],
      [
        "env",
        "-ia",
        "foo",
        "-S",
        "A=1\\_CARGO_TARGET_DIR=/x cargo x test -p a",
      ],
      [
        "env",
        "-P",
        "/bin",
        "-S",
        "A=1\\_CARGO_TARGET_DIR=/x cargo x test -p a",
      ],
      // The raw scan also sees an assignment after a \\_ separator.
      ["setsid", "x", "A=1\\_CARGO_TARGET_DIR=/x"],
    ]
  ) rejected(argv, "never set CARGO_TARGET_DIR");
  for (
    const argv of [
      ["env", "-P", "/bin", "cargo", "x", "test", "-p", "a"],
      ["env", "--bogus", "deno", "test"],
      ["timeout", "60", "env", "-L", "u", "cargo", "x", "lint"],
    ]
  ) rejected(argv, "unsupported env option");
  // An `env` that is an ordinary argument elsewhere in argv is not rejected
  // for its following tokens.
  const grep = checkPolicyViolations(["grep", "-w", "env", "-x", "f"]);
  assert(grep.length === 0, JSON.stringify(grep));
});

Deno.test("check policy rejects env --env0-from, whose entries it cannot read", () => {
  for (
    const argv of [
      ["env", "--env0-from=f", "cargo", "x", "test", "-p", "a"],
      ["env", "--env0-from", "f", "cargo", "x", "test", "-p", "a"],
      ["env", "--env0", "f", "deno", "test"],
      ["setsid", "env", "--env0-from=f", "deno", "test"],
    ]
  ) rejected(argv, "--env0-from");
});

Deno.test("check policy rejects any element containing a target-dir assignment", () => {
  // Intended: every element is scanned, so a search pattern that only mentions
  // the assignment is rejected too; search for the name without the `=`.
  rejected(
    ["grep", "-rn", "CARGO_TARGET_DIR=", "docs"],
    "never set CARGO_TARGET_DIR",
  );
  const name = checkPolicyViolations([
    "grep",
    "-rn",
    "CARGO_TARGET_DIR",
    "docs",
  ]);
  assert(name.length === 0, JSON.stringify(name));
});
