/**
 * Repository test policy for plan checks, enforced when plan evidence is
 * submitted. Persisted plans, receipts and verifications never pass through it,
 * so records accepted before the policy existed keep parsing and replaying.
 */
import type { Check } from "./lifecycle.ts";

/** One paragraph shared by planning and plan-review prompts. */
export const CHECK_POLICY_SUMMARY =
  "Check policy (enforced when plan evidence is submitted): each check's command is an argv array the lifecycle runs directly in the issue worktree without a shell. Put every argument in its own element; do not use shell operators (&&, ||, |, ;, &, redirections), sh/bash -c wrappers, $VARIABLES or ~ paths, because nothing is expanded. Set environment variables only through a leading env NAME=VALUE, and never set CARGO_TARGET_DIR, CARGO_BUILD_BUILD_DIR, --target-dir or a target-dir or build-dir config anywhere in argv (any element containing CARGO_TARGET_DIR= is rejected, even a grep pattern; search for the name without the =). Use only env's -i, -0, -v, -u, -C, -S and -a options; other env options and --env0-from are rejected. Cargo checks must be focused: cargo x test, cargo test and cargo nextest run need a package (-p) or a test selection (-E filterset, a test name, or a named --test, --bench, --bin or --example target), and --workspace (or a package glob such as -p '*' or -p 'yinz-*') does not count as a package, nor does an empty name, -E 'all()', -E 'not none()' or a --skip pattern; cargo x test and cargo x integration-test pass everything after -- to nextest as nextest options, so the values of options such as --no-tests or --run-ignored there are not test names, and with an explicit -- every nextest option (such as -p or -E) must come after it, because xtask rejects flags it does not own before the --; cargo x integration-test needs a selection and -j N (or --serial); cargo x format needs --check; cargo x lint must not use --fix; cargo x build is allowed; cargo x ci is the merge gate the lifecycle runs at merge; cargo x run (and the cargo q alias) launches the interactive app, and a check must terminate; other cargo x subcommands are not development checks. The xtask rules also apply to cargo run of the xtask package (named by -p, --bin, -m/--manifest-path or a working directory inside xtask) and to the repository's cargo aliases, and to the xtask binary itself (target/debug/xtask), and the nextest rules to the cargo-nextest binary. Never define cargo aliases (--config alias.*, CARGO_ALIAS_*) or pass --config files in a check. Use nextest options such as --no-capture and -j N, never the libtest flags --nocapture or --test-threads. Wrappers such as timeout, nice or env -C do not exempt a cargo command from these rules.";

const SHELL_OPERATORS = new Set([
  "&&",
  "||",
  "|",
  "|&",
  ";",
  "&",
  ">",
  ">>",
  "<",
  "2>",
  "2>&1",
]);
const SHELLS = new Set([
  "sh",
  "bash",
  "zsh",
  "dash",
  "ksh",
  "fish",
  "csh",
  "tcsh",
]);
/** Variables that move the target or build directory out of the worktree. */
const TARGET_DIR_VARIABLES = new Set([
  "CARGO_TARGET_DIR",
  "CARGO_BUILD_TARGET_DIR",
  "CARGO_BUILD_BUILD_DIR",
]);
/** Cargo reads `CARGO_ALIAS_<NAME>` as an alias that would hide the command. */
const ALIAS_VARIABLE = "CARGO_ALIAS_";
const ASSIGNMENT = /^[A-Za-z_][A-Za-z0-9_]*=/;
/**
 * A forbidden NAME=VALUE anywhere in an element, such as in env -S text,
 * including an -S string attached to its flag (-SCARGO_TARGET_DIR=...) or
 * after env -S's `\_` separator. This scan also rejects an element that only
 * mentions the assignment, such as a grep pattern `CARGO_TARGET_DIR=`.
 */
const FORBIDDEN_ASSIGNMENT =
  /(?:^-[i0v]*S|(?<![A-Za-z0-9_])|(?<=\\_))(CARGO_TARGET_DIR|CARGO_BUILD_TARGET_DIR|CARGO_BUILD_BUILD_DIR|CARGO_ALIAS_[A-Za-z0-9_]+)=/g;
/**
 * Flags that take a value in cargo, nextest or xtask selection arguments.
 * Mirrors `cargo nextest run --help` (nextest 0.9.148) plus cargo test's own.
 * The libtest flags at the end are not top-level nextest options (nextest
 * rejects them before `--`), but xtask forwards the tokens after its `--` to
 * nextest at top level, so their values must not count as test names there.
 */
const VALUED_FLAGS = new Set([
  "-p",
  "--package",
  "-E",
  "--filterset",
  "--filter-expr",
  "-j",
  "--jobs",
  "--test-threads",
  "-P",
  "--profile",
  "--retries",
  "--run-ignored",
  "-F",
  "--features",
  "--target",
  "--target-dir",
  "--partition",
  "--message-format",
  "--message-format-version",
  "--cargo-message-format",
  "--cargo-profile",
  "--bin",
  "--test",
  "--bench",
  "--example",
  "--exclude",
  "--build-jobs",
  "--no-tests",
  "-R",
  "--rerun",
  "--platform-filter",
  "--flaky-result",
  "--debugger",
  "--tracer",
  "--stress-count",
  "--stress-duration",
  "--status-level",
  "--final-status-level",
  "--failure-output",
  "--success-output",
  "--show-progress",
  "--max-progress-running",
  "--max-fail",
  "--archive-file",
  "--archive-format",
  "--extract-to",
  "--cargo-metadata",
  "--workspace-remap",
  "--binaries-metadata",
  "--target-dir-remap",
  "--build-dir-remap",
  "--config-file",
  "--user-config-file",
  "--tool-config-file",
  "-m",
  "--manifest-path",
  "--lockfile-path",
  "--color",
  "--config",
  "-Z",
  "--skip",
  "--logfile",
  "--format",
  "--shuffle-seed",
]);
/**
 * Test-binary (libtest-style) flags after `--` that take a value. `--skip`
 * excludes tests, so its value never counts as a selection.
 */
const BINARY_VALUED_FLAGS = new Set([
  "--skip",
  "--test-threads",
  "--logfile",
  "--format",
  "--color",
  "--shuffle-seed",
  "-Z",
]);
/** Cargo's global flags that take a value before the subcommand. */
const GLOBAL_VALUED_FLAGS = new Set([
  "--config",
  "-Z",
  "-C",
  "--color",
  "-m",
  "--manifest-path",
]);
/** Options naming one test target; each narrows a run like a package does. */
const TARGET_SELECTORS = new Set(["--test", "--bench", "--example", "--bin"]);
/** A package or target value that selects everything (`*`), or nothing at all. */
const BROAD_GLOB = /^[*?]*$/;
/**
 * A package glob such as `yinz-*` names (nearly) every package, so any glob
 * counts as broad; name packages explicitly.
 */
const PACKAGE_GLOB = /[*?[]/;
/** A filterset that matches every test (or is empty) is not a selection. */
const BROAD_FILTERSET =
  /^\s*(?:(?:all|true)\(\s*\)|(?:not\s+|!\s*)none\(\s*\))?\s*$/;
/**
 * The flags xtask itself owns for its nextest-forwarding commands, mirroring
 * `FORWARDING_COMMANDS` in xtask/src/main.rs.
 */
const XTASK_OWNED_FLAGS: Record<string, string[]> = {
  test: ["-h", "--help"],
  "integration-test": ["--serial", "-h", "--help"],
};
const XTASK_ALLOWED =
  "allowed: test, integration-test, lint, format --check, build";
/**
 * The repository's cargo aliases, mirroring `[alias]` in .cargo/config.toml
 * (a unit test fails when the two drift). `x` and `xtask` stay unexpanded:
 * the xtask rules match them, and `run --package xtask --` is recognized too.
 */
export const CARGO_ALIASES: Record<string, string> = {
  xtask: "run --package xtask --",
  x: "xtask",
  q: "x run --quick-run",
  make: "xtask deprecated",
};
const XTASK_SUBCOMMANDS = new Set(["x", "xtask"]);
const XTASK = "xtask";
/** Cargo run options that take a value, used to find `--package xtask`. */
const RUN_VALUED_FLAGS = new Set([
  "-p",
  "--package",
  "--bin",
  "--example",
  "-F",
  "--features",
  "-j",
  "--jobs",
  "--profile",
  "--target",
  "--target-dir",
  "-m",
  "--manifest-path",
  "--lockfile-path",
  "--message-format",
  "--color",
  "--config",
  "-Z",
]);
const MANIFEST_FLAGS = new Set(["-m", "--manifest-path"]);
/** Bounds alias expansion; cargo itself rejects recursive aliases. */
const ALIAS_DEPTH = 8;

function basename(path: string) {
  return path.split("/").pop() ?? path;
}

function quoted(index: number, value: string) {
  return `argv[${index}] ${JSON.stringify(value)}`;
}

type ParsedOption = {
  name: string;
  /** The value is part of the token (`--name=value`, `-xVALUE`, `-x=VALUE`). */
  attached: boolean;
  /** The attached value, or else the next token (when the option takes one). */
  value: string | undefined;
};

/** Splits one option token the way clap does, including the `-x=VALUE` form. */
function parseOption(token: string, next: string | undefined): ParsedOption {
  if (token.startsWith("--")) {
    const equals = token.indexOf("=");
    return equals < 0 ? { name: token, attached: false, value: next } : {
      name: token.slice(0, equals),
      attached: true,
      value: token.slice(equals + 1),
    };
  }
  const rest = token.slice(2);
  return rest
    ? {
      name: token.slice(0, 2),
      attached: true,
      value: rest.startsWith("=") ? rest.slice(1) : rest,
    }
    : { name: token, attached: false, value: next };
}

function isOption(token: string) {
  return token.startsWith("-") && token !== "-";
}

/** Path components with `.` dropped and `..` resolved where possible. */
function components(path: string): string[] {
  const parts: string[] = [];
  for (const part of path.split("/")) {
    if (!part || part === ".") continue;
    if (part === ".." && parts.length && parts.at(-1) !== "..") parts.pop();
    else parts.push(part);
  }
  return parts;
}

function joinPath(base: string, path: string) {
  return path.startsWith("/") ? path : `${base}/${path}`;
}

/** Shell-style glob match (`*`, `?`, `[...]`), as cargo accepts for specs. */
function globMatches(pattern: string, name: string) {
  if (!/[*?[]/.test(pattern)) return pattern === name;
  const source = pattern
    .replace(/[.+^${}()|\\]/g, "\\$&")
    .replace(/\*/g, ".*")
    .replace(/\?/g, ".")
    .replace(/\[!/g, "[^");
  try {
    return new RegExp(`^${source}$`).test(name);
  } catch {
    return true;
  }
}

/**
 * The package name in a cargo package spec: `name`, `name@ver`, `name:ver`,
 * `url`, `url#ver`, `url#name` or `url#name@ver` (a URL names its last path
 * component, as in `path+file:///repo/xtask`).
 */
function packageName(spec: string): string {
  const hash = spec.lastIndexOf("#");
  const url = hash >= 0
    ? spec.slice(0, hash)
    : spec.includes("://")
    ? spec
    : undefined;
  let name = hash >= 0 ? spec.slice(hash + 1) : url === undefined ? spec : "";
  name = name.split("@")[0].split(":")[0];
  if ((!name || /^v?\d/.test(name)) && url !== undefined) {
    const path = url.replace(/^[A-Za-z+-]+:\/\//, "").split("?")[0];
    name = (components(path).at(-1) ?? "").replace(/\.git$/, "");
  }
  return name;
}

type Selection = {
  packages: boolean;
  workspace: boolean;
  filterset: boolean;
  positional: boolean;
  targets: boolean;
  jobs: boolean;
  flags: Set<string>;
};

/** Classifies selection arguments; an unknown valued flag only adds permission. */
function selection(tokens: string[]): Selection {
  const result: Selection = {
    packages: false,
    workspace: false,
    filterset: false,
    positional: false,
    targets: false,
    jobs: false,
    flags: new Set(),
  };
  for (let index = 0; index < tokens.length; index++) {
    const token = tokens[index];
    if (token === "--") {
      // Remaining tokens go to the test binary: names select tests; flags,
      // their values and --skip patterns (which exclude tests) do not.
      for (let rest = index + 1; rest < tokens.length; rest++) {
        const arg = tokens[rest];
        if (!arg.startsWith("-")) result.positional ||= arg !== "";
        else if (BINARY_VALUED_FLAGS.has(arg)) rest++;
      }
      break;
    }
    if (!isOption(token)) {
      // An empty substring filter matches every test.
      result.positional ||= token !== "";
      continue;
    }
    const { name, attached, value } = parseOption(token, tokens[index + 1]);
    result.flags.add(name);
    const narrow = !BROAD_GLOB.test(value ?? "");
    if (name === "-p" || name === "--package") {
      // cargo adds every package spec, so one broad glob selects them all.
      if (value && !PACKAGE_GLOB.test(value)) result.packages = true;
      else result.workspace = true;
    }
    if (TARGET_SELECTORS.has(name) && narrow) result.targets = true;
    if (name === "--workspace" || name === "--all") result.workspace = true;
    if (
      ["-E", "--filterset", "--filter-expr"].includes(name) &&
      !BROAD_FILTERSET.test(value ?? "")
    ) {
      result.filterset = true;
    }
    if (name === "-j" || name === "--jobs") result.jobs = true;
    if (!attached && VALUED_FLAGS.has(name)) index++;
  }
  return result;
}

function selected(tokens: string[]) {
  const s = selection(tokens);
  return (s.packages && !s.workspace) || s.filterset || s.positional ||
    s.targets;
}

const UNFILTERED =
  "requires a package (-p) or test selection (-E filterset, a test name or a --test target); unfiltered suites are not development checks";

/** The rules for one cargo command line (the args after `cargo`). */
function cargoViolations(args: string[], chdirs: string[]): string[] {
  const violations: string[] = [];
  for (const [index, token] of args.entries()) {
    if (token === "--target-dir" || token.startsWith("--target-dir=")) {
      violations.push("never set CARGO_TARGET_DIR or --target-dir");
    }
    const config = token === "--config"
      ? args[index + 1]
      : token.startsWith("--config=")
      ? token.slice("--config=".length)
      : undefined;
    if (config !== undefined) {
      if (config.includes("target-dir")) {
        violations.push("never change build.target-dir through --config");
      }
      if (config.includes("build-dir")) {
        violations.push("never change build.build-dir through --config");
      }
      if (/\balias\b/.test(config)) {
        violations.push(
          "never define cargo aliases through --config; an alias hides the command from this policy",
        );
      } else if (!config.includes("=")) {
        violations.push(
          "--config files are not allowed in checks; they can define aliases or change the target directory",
        );
      }
    }
    if (
      token === "--nocapture" || token === "--test-threads" ||
      token.startsWith("--test-threads=")
    ) {
      violations.push(
        `${token} is a libtest flag; use nextest's --no-capture or -j N`,
      );
    }
  }
  const global = leadingOptions(
    args[0]?.startsWith("+") ? args.slice(1) : args,
  );
  // cargo -C (nightly) changes directory like env -C.
  const dirs = [
    ...chdirs,
    ...global.options.filter((o) => o.name === "-C").map((o) => o.value ?? ""),
  ];
  let rest = global.rest;
  for (
    let depth = 0;
    depth < ALIAS_DEPTH && Object.hasOwn(CARGO_ALIASES, rest[0] ?? "") &&
    !XTASK_SUBCOMMANDS.has(rest[0]);
    depth++
  ) {
    rest = [...CARGO_ALIASES[rest[0]].split(/\s+/), ...rest.slice(1)];
  }
  const [subcommand, ...tail] = rest;
  const xtaskArgs = XTASK_SUBCOMMANDS.has(subcommand)
    ? tail
    : subcommand === "run" || subcommand === "r"
    ? xtaskRunArgs(
      tail,
      dirs,
      global.options.findLast((o) => MANIFEST_FLAGS.has(o.name))?.value,
    )
    : undefined;
  if (xtaskArgs) {
    violations.push(...xtaskViolations(xtaskArgs));
  } else if (subcommand === "test" || subcommand === "t") {
    if (!selected(tail)) violations.push(`cargo test ${UNFILTERED}`);
  } else if (subcommand === "nextest") {
    const [run, ...forwarded] = leadingOptions(tail).rest;
    if ((run === "run" || run === "r") && !selected(forwarded)) {
      violations.push(`cargo nextest run ${UNFILTERED}`);
    }
  }
  return violations;
}

/**
 * Splits leading options (with the values of known valued ones) from the
 * subcommand and its arguments.
 */
function leadingOptions(args: string[]) {
  const options: ParsedOption[] = [];
  let index = 0;
  while (index < args.length && isOption(args[index]) && args[index] !== "--") {
    const parsed = parseOption(args[index], args[index + 1]);
    const valued = GLOBAL_VALUED_FLAGS.has(parsed.name) ||
      VALUED_FLAGS.has(parsed.name);
    if (!parsed.attached && !valued) parsed.value = undefined;
    options.push(parsed);
    index += !parsed.attached && valued ? 2 : 1;
  }
  return { options, rest: args.slice(index) };
}

/** Whether a directory a check changes into lies inside the xtask package. */
function insideXtask(dirs: string[]) {
  const cumulative = dirs.reduce((base, dir) => joinPath(base, dir), ".");
  return [...dirs, cumulative].some((dir) => components(dir).includes(XTASK));
}

/** Whether a manifest path, read from any changed directory, is xtask's. */
function xtaskManifest(manifest: string, dirs: string[]) {
  const cumulative = dirs.reduce((base, dir) => joinPath(base, dir), ".");
  return [manifest, ...[...dirs, cumulative].map((d) => joinPath(d, manifest))]
    .some((path) => {
      const parts = components(path);
      return parts.at(-1) === "Cargo.toml" && parts.at(-2) === XTASK;
    });
}

/**
 * The xtask command line when `cargo run` targets the xtask package (as the
 * `xtask` alias does), or undefined for any other `cargo run`. A package
 * spec, binary, manifest path (`-m`) or working directory can name xtask.
 */
function xtaskRunArgs(
  args: string[],
  dirs: string[],
  globalManifest: string | undefined,
): string[] | undefined {
  const separator = args.indexOf("--");
  const options = separator < 0 ? args : args.slice(0, separator);
  let named = false;
  let xtask = false;
  let manifest = globalManifest;
  for (let index = 0; index < options.length; index++) {
    const token = options[index];
    if (!isOption(token)) continue;
    const { name, attached, value } = parseOption(token, options[index + 1]);
    if (!attached && RUN_VALUED_FLAGS.has(name)) index++;
    if (name === "-p" || name === "--package") {
      named = true;
      if (globMatches(packageName(value ?? ""), XTASK)) xtask = true;
    } else if (name === "--bin") {
      named = true;
      if (globMatches(value ?? "", XTASK)) xtask = true;
    } else if (MANIFEST_FLAGS.has(name)) {
      manifest = value ?? "";
    }
  }
  // Without a package or binary, cargo runs the manifest's (or the current
  // directory's) package.
  if (!named) {
    xtask = manifest === undefined
      ? insideXtask(dirs)
      : xtaskManifest(manifest, dirs);
  }
  if (!xtask) return undefined;
  return separator < 0 ? [] : args.slice(separator + 1);
}

/**
 * Splits a forwarding xtask command's arguments the way xtask does: the
 * flags xtask owns, and the arguments it appends to `cargo nextest run` as
 * top-level nextest options. With a `--` in argv, xtask leaves argv alone and
 * xflags drops that first `--` (later ones reach nextest); without one, xtask
 * inserts it before the first token it does not own. Tokens before an explicit
 * `--` that xtask does not own count as forwarded: xflags forwards positional
 * ones and rejects unknown flags, which fails the check anyway.
 */
function xtaskForwarding(args: string[], owned: string[]) {
  const separator = args.indexOf("--");
  if (separator >= 0) {
    const before = args.slice(0, separator);
    return {
      own: before.filter((arg) => owned.includes(arg)),
      forwarded: [
        ...before.filter((arg) => !owned.includes(arg)),
        ...args.slice(separator + 1),
      ],
    };
  }
  const first = args.findIndex((arg) => !owned.includes(arg));
  return first < 0
    ? { own: args, forwarded: [] }
    : { own: args.slice(0, first), forwarded: args.slice(first) };
}

/** Flags before an explicit `--` that xtask does not own (xflags rejects them). */
function earlyForwardedFlags(args: string[], owned: string[]) {
  const separator = args.indexOf("--");
  return separator < 0
    ? []
    : args.slice(0, separator).filter((arg) =>
      isOption(arg) && !owned.includes(arg)
    );
}

/** The xtask rules for one `cargo x` command line (the args after `x`). */
function xtaskViolations(args: string[]): string[] {
  const [xtask, ...rest] = args;
  const forwarding = Object.hasOwn(XTASK_OWNED_FLAGS, xtask ?? "");
  const forwarded = forwarding
    ? xtaskForwarding(rest, XTASK_OWNED_FLAGS[xtask])
    : { own: [], forwarded: rest };
  if (xtask === "ci") {
    return ["cargo x ci is the merge gate; the lifecycle runs it at merge"];
  }
  // With an explicit `--`, xtask leaves argv alone and xflags rejects every
  // flag before it that xtask does not own, so the check would fail to run.
  const early = forwarding
    ? earlyForwardedFlags(rest, XTASK_OWNED_FLAGS[xtask]).map((flag) =>
      `cargo x ${xtask} rejects ${flag} before an explicit --; put nextest options after the -- (cargo x ${xtask} -- ${flag} ...) or drop the --`
    )
    : [];
  if (xtask === "test") {
    return selected(forwarded.forwarded)
      ? early
      : [...early, `cargo x test ${UNFILTERED}`];
  }
  if (xtask === "integration-test") {
    const violations: string[] = [...early];
    const s = selection(forwarded.forwarded);
    if (!s.filterset && !s.positional) {
      violations.push(
        "cargo x integration-test requires a test selection (-E filterset or a test name)",
      );
    }
    if (!s.jobs && !forwarded.own.includes("--serial")) {
      violations.push(
        "cargo x integration-test requires -j N (or --serial) to bound workers",
      );
    }
    return violations;
  }
  if (xtask === "format") {
    return rest.includes("--check")
      ? []
      : ["cargo x format rewrites files; use cargo x format --check"];
  }
  if (xtask === "lint") {
    return rest.includes("--fix")
      ? ["cargo x lint --fix rewrites files; drop --fix"]
      : [];
  }
  if (xtask === "run") {
    return [
      `cargo x run is not a focused development check: it launches the interactive app, and a check must terminate (${XTASK_ALLOWED})`,
    ];
  }
  if (xtask === "build") return [];
  return [
    `${
      xtask === undefined ? "cargo x" : `cargo x ${xtask}`
    } is not a focused development check (${XTASK_ALLOWED})`,
  ];
}

/** Options of transparent wrappers that run the rest of argv as a command. */
const WRAPPERS: Record<string, { valued: Set<string>; operands: number }> = {
  timeout: {
    valued: new Set(["-k", "--kill-after", "-s", "--signal"]),
    operands: 1,
  },
  nice: { valued: new Set(["-n", "--adjustment"]), operands: 0 },
  ionice: {
    valued: new Set(["-c", "--class", "-n", "--classdata"]),
    operands: 0,
  },
  time: {
    valued: new Set(["-o", "--output", "-f", "--format"]),
    operands: 0,
  },
  stdbuf: {
    valued: new Set(["-i", "--input", "-o", "--output", "-e", "--error"]),
    operands: 0,
  },
  nohup: { valued: new Set(), operands: 0 },
};
/** Marks env --env0-from, which loads assignments from a file. */
const ENV_FILE = "env0-from";
/**
 * GNU env's long options (coreutils 9.12), which getopt also accepts as
 * unambiguous prefixes. The short letter is set for options that take a value
 * (`a`, `u`, `C`, `S`, and --env0-from, which has no short form). The signal
 * options take an optional value only in the attached `--name=SIG` form.
 */
const ENV_LONG_OPTIONS: Record<string, string | undefined> = {
  "--argv0": "a",
  "--env0-from": ENV_FILE,
  "--ignore-environment": undefined,
  "--null": undefined,
  "--unset": "u",
  "--chdir": "C",
  "--split-string": "S",
  "--debug": undefined,
  "--list-signal-handling": undefined,
  "--ignore-signal": undefined,
  "--default-signal": undefined,
  "--block-signal": undefined,
};
/** Boolean short options that can be combined, as in -iv or -ivS... */
const ENV_SHORT_FLAGS = "i0v";
/** Short options that take a value, attached or as the next token. */
const ENV_SHORT_VALUED = "auCS";
/** Bounds nested wrappers, including env -S strings that expand to more env. */
const WRAPPER_DEPTH = 16;

/** What env invocations set up for the command they run. */
type EnvFacts = {
  assigned: string[];
  unset: string[];
  chdirs: string[];
  /** Every token produced by splitting an -S string. */
  split: string[];
  /** Files env --env0-from loads assignments from. */
  envFiles: string[];
  /** Env options this parser does not know, where parsing stopped. */
  unknown: string[];
  /**
   * Commands from -S strings found after an unknown env option, which a
   * stopped parser would otherwise miss.
   */
  extra: string[][];
};

/**
 * Splits env -S text as GNU env does: whitespace separates words; single and
 * double quotes group; `\_` separates outside double quotes (a space inside
 * them); backslash escapes; `#` at a word start and `\c` end the string.
 */
function splitString(text: string): string[] {
  const words: string[] = [];
  let word = "";
  let started = false;
  let single = false;
  let double = false;
  const finish = () => {
    if (started) words.push(word);
    word = "";
    started = false;
  };
  const escapes: Record<string, string> = {
    f: "\f",
    n: "\n",
    r: "\r",
    t: "\t",
    v: "\v",
  };
  for (let index = 0; index < text.length; index++) {
    const char = text[index];
    if (!single && !double && /\s/.test(char)) {
      finish();
    } else if (char === "'" && !double) {
      single = !single;
      started = true;
    } else if (char === '"' && !single) {
      double = !double;
      started = true;
    } else if (char === "#" && !single && !double && !started) {
      break;
    } else if (
      char === "\\" &&
      (!single || text[index + 1] === "\\" || text[index + 1] === "'")
    ) {
      const next = text[++index] ?? "";
      if (next === "c") break;
      if (next === "_" && !double) {
        finish();
        continue;
      }
      word += next === "_" ? " " : escapes[next] ?? next;
      started = true;
    } else {
      word += char;
      started = true;
    }
  }
  finish();
  return words;
}

/** Resolves a GNU env long option (or unambiguous prefix) to its full name. */
function envLongOption(name: string): string | undefined {
  if (Object.hasOwn(ENV_LONG_OPTIONS, name)) return name;
  const matches = Object.keys(ENV_LONG_OPTIONS).filter((option) =>
    option.startsWith(name)
  );
  return matches.length === 1 ? matches[0] : undefined;
}

/** The -S value an env token carries, if it is an -S or --split-string. */
function splitStringOption(token: string, next: string | undefined) {
  if (token.startsWith("--")) {
    const equals = token.indexOf("=");
    const name = envLongOption(equals < 0 ? token : token.slice(0, equals));
    if (name !== "--split-string") return undefined;
    return equals < 0 ? next ?? "" : token.slice(equals + 1);
  }
  const short = token.match(/^-[i0v]*S(.*)$/s);
  if (!short) return undefined;
  return short[1] || (next ?? "");
}

/**
 * Fails closed when env parsing stops at an unknown option: every later -S
 * string is still split, so its assignments and command are inspected.
 */
function scanAfterUnknown(args: string[], facts: EnvFacts) {
  for (let index = 0; index < args.length; index++) {
    const value = splitStringOption(args[index], args[index + 1]);
    if (value === undefined) continue;
    const split = splitString(value);
    facts.split.push(...split);
    facts.extra.push(split);
  }
}

/** The command one env invocation runs, collecting what it sets up. */
function envCommand(command: string[], facts: EnvFacts): string[] {
  let args = command.slice(1);
  let index = 0;
  for (let rounds = 0; index < args.length && rounds < 64; rounds++) {
    const token = args[index];
    if (token === "--") return args.slice(index + 1);
    // A lone `-` implies -i.
    if (token === "-") {
      index++;
      continue;
    }
    let letter: string | undefined;
    let value: string | undefined;
    let consumed = 1;
    if (token.startsWith("--")) {
      const equals = token.indexOf("=");
      const name = envLongOption(equals < 0 ? token : token.slice(0, equals));
      if (name === undefined) {
        facts.unknown.push(token);
        scanAfterUnknown(args.slice(index + 1), facts);
        break;
      }
      letter = ENV_LONG_OPTIONS[name];
      if (letter === undefined) {
        index++;
        continue;
      }
      if (equals >= 0) value = token.slice(equals + 1);
      else {
        value = args[index + 1];
        consumed = 2;
      }
    } else if (isOption(token)) {
      let position = 1;
      while (
        position < token.length && ENV_SHORT_FLAGS.includes(token[position])
      ) position++;
      if (position === token.length) {
        index++;
        continue;
      }
      if (!ENV_SHORT_VALUED.includes(token[position])) {
        facts.unknown.push(token);
        scanAfterUnknown(args.slice(index + 1), facts);
        break;
      }
      letter = token[position];
      const rest = token.slice(position + 1);
      if (rest) value = rest;
      else {
        value = args[index + 1];
        consumed = 2;
      }
    } else if (ASSIGNMENT.test(token)) {
      facts.assigned.push(token.slice(0, token.indexOf("=")));
      index++;
      continue;
    } else break;
    if (letter === ENV_FILE) facts.envFiles.push(value ?? "");
    if (letter === "u") facts.unset.push(value ?? "");
    if (letter === "C") facts.chdirs.push(value ?? "");
    if (letter === "S") {
      // env splits the string into further env arguments, including the command.
      const split = splitString(value ?? "");
      facts.split.push(...split);
      args = [...split, ...args.slice(index + consumed)];
      index = 0;
      continue;
    }
    index += consumed;
  }
  return args.slice(index);
}

/** Skips a wrapper's options and fixed operands (such as timeout's duration). */
function wrappedCommand(command: string[]): string[] {
  const { valued, operands } = WRAPPERS[basename(command[0])];
  let index = 1;
  while (index < command.length && command[index].startsWith("-")) {
    const token = command[index];
    if (token === "--") {
      index++;
      break;
    }
    const long = token.startsWith("--");
    const attached = long ? token.includes("=") : token.length > 2;
    const name = long ? token.split("=")[0] : token.slice(0, 2);
    index += !attached && valued.has(name) ? 2 : 1;
  }
  return command.slice(index + operands);
}

function isWrapper(token: string) {
  const name = basename(token);
  return name === "env" || Object.hasOwn(WRAPPERS, name);
}

/**
 * Removes leading env and transparent wrappers (timeout, nice, ionice, time,
 * stdbuf, nohup), collecting what env sets up.
 */
function unwrap(argv: string[], facts: EnvFacts): string[] {
  let command = argv;
  for (let depth = 0; depth < WRAPPER_DEPTH && command.length; depth++) {
    const name = basename(command[0]);
    if (name === "env") command = envCommand(command, facts);
    else if (Object.hasOwn(WRAPPERS, name)) command = wrappedCommand(command);
    else break;
  }
  return command;
}

function forbiddenAssignments(element: string) {
  return [...element.matchAll(FORBIDDEN_ASSIGNMENT)].map((match) => match[1]);
}

/** Pure and deterministic: every policy violation in one check's argv. */
export function checkPolicyViolations(argv: string[]): string[] {
  const violations: string[] = [];
  if (/\s/.test(argv[0] ?? "")) {
    violations.push(
      `${quoted(0, argv[0])}: split the command into separate argv elements`,
    );
  }
  for (const [index, element] of argv.entries()) {
    if (SHELL_OPERATORS.has(element)) {
      violations.push(
        `${quoted(index, element)}: shell operator; checks run without a shell`,
      );
    } else if (/\s(&&|\|\|)\s/.test(element)) {
      violations.push(
        `${quoted(index, element)}: embedded shell command list`,
      );
    }
    if (
      SHELLS.has(basename(element)) &&
      /^-[A-Za-z]*c[A-Za-z]*$/.test(argv[index + 1] ?? "")
    ) {
      violations.push(`${quoted(index, element)}: shell -c wrapper`);
    }
    if (
      /^\$(\{|[A-Za-z_])/.test(element) || element === "~" ||
      element.startsWith("~/")
    ) {
      violations.push(
        `${quoted(index, element)}: not expanded without a shell`,
      );
    }
  }
  const facts: EnvFacts = {
    assigned: [],
    unset: [],
    chdirs: [],
    split: [],
    envFiles: [],
    unknown: [],
    extra: [],
  };
  let command = argv;
  if (ASSIGNMENT.test(argv[0] ?? "")) {
    violations.push(
      `${quoted(0, argv[0])}: environment assignment requires env`,
    );
    let index = 0;
    while (index < argv.length && ASSIGNMENT.test(argv[index])) index++;
    command = argv.slice(index);
  }
  // Every env or wrapper in argv is unwrapped, not only a leading one, so an
  // unknown runner (setsid, xargs, rustup run) cannot hide an env -S string.
  const commands = [unwrap(command, facts)];
  // An env option the parser does not know stops the leading command's
  // parsing, so it is rejected rather than trusted. (Elsewhere in argv an
  // `env` may be an ordinary argument; its -S strings are still inspected.)
  for (const option of new Set(facts.unknown)) {
    violations.push(
      `env ${option}: unsupported env option in a check; use only -i, -0, -v, -u, -C, -S, -a and NAME=VALUE`,
    );
  }
  for (let index = 1; index < argv.length; index++) {
    if (isWrapper(argv[index])) {
      commands.push(unwrap(argv.slice(index), facts));
    }
  }
  for (let index = 0; index < facts.extra.length && index < 64; index++) {
    commands.push(unwrap(facts.extra[index], facts));
  }
  if (facts.envFiles.length) {
    violations.push(
      "env --env0-from loads environment entries from a file this policy cannot inspect; set variables as env NAME=VALUE",
    );
  }
  // Forbidden assignments are found wherever they appear (behind any runner,
  // inside env -S strings or shell scripts), independent of unwrapping.
  const assigned = [
    ...argv.flatMap(forbiddenAssignments),
    ...facts.split.flatMap(forbiddenAssignments),
    ...facts.assigned,
  ];
  for (const name of new Set([...assigned, ...facts.unset])) {
    if (TARGET_DIR_VARIABLES.has(name)) {
      violations.push(
        `${name}: never set CARGO_TARGET_DIR or move the target or build directory`,
      );
    }
  }
  for (const name of new Set(assigned)) {
    if (name.startsWith(ALIAS_VARIABLE)) {
      violations.push(
        `${name}: never define cargo aliases in a check; an alias hides the command from this policy`,
      );
    }
  }
  // Known wrappers are already removed. Every cargo later in a command
  // (behind xargs, an env option this parser does not know, another runner or
  // a decoy argument named cargo) still gets the cargo rules, so no wrapper
  // can hide the merge gate or an unfiltered suite. cargo-nextest run
  // directly behaves as `cargo nextest`. The xtask binary, which any earlier
  // `cargo x` builds, gets the xtask rules where it is the command, or
  // wherever its build-output path (under a `target` directory) appears; a
  // bare `xtask` elsewhere is usually a package name (`-p xtask`).
  for (const candidate of commands) {
    for (const [index, token] of candidate.entries()) {
      const name = basename(token);
      if (
        (name === XTASK || name === "xtask.exe") &&
        (index === 0 || components(token).slice(0, -1).includes("target"))
      ) {
        violations.push(...xtaskViolations(candidate.slice(index + 1)));
        continue;
      }
      if (name !== "cargo" && name !== "cargo-nextest") continue;
      const args = candidate.slice(index + 1);
      violations.push(
        ...cargoViolations(
          name === "cargo-nextest"
            ? ["nextest", ...(args[0] === "nextest" ? args.slice(1) : args)]
            : args,
          facts.chdirs,
        ),
      );
    }
  }
  return [...new Set(violations)];
}

/** Every violation in a plan's checks, labelled with check index and id. */
export function checkPlanPolicy(checks: Check[]): string[] {
  return checks.flatMap((check, index) =>
    checkPolicyViolations(check.command).map((reason) =>
      `checks[${index}] (id ${JSON.stringify(check.id)}): ${reason}`
    )
  );
}

/** Rejects new plan evidence whose checks violate the repository test policy. */
export function assertCheckPolicy(checks: Check[]): void {
  const violations = checkPlanPolicy(checks);
  if (violations.length) {
    throw new Error(
      `Plan checks violate repository test policy:\n- ${
        violations.join("\n- ")
      }`,
    );
  }
}
