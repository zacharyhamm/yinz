import { model } from "./yinz_install.ts";
import type { Context } from "./_lib/workflow.ts";

function assert(value: unknown): asserts value {
  if (!value) throw new Error("assertion failed");
}

Deno.test("install remembers destinations, passes literal arguments, and requires exit status zero", async () => {
  const dir = await Deno.makeTempDir({ prefix: "install ' $() " });
  const original = Deno.Command;
  const home = Deno.env.get("HOME");
  const saved = new Map<string, Record<string, unknown>>();
  const calls: { command: string | URL; args: string[] }[] = [];
  let response: Record<string, unknown> = { exited: true, exit_status: 0 };
  let failure = false;
  const c: Context = {
    repoDir: dir,
    globalArgs: {},
    signal: new AbortController().signal,
    logger: { info() {} },
    readResource: (name) => Promise.resolve(saved.get(name) ?? null),
    writeResource: (spec, name, value) => {
      model.resources[spec as keyof typeof model.resources].schema.parse(value);
      saved.set(name, structuredClone(value));
      return Promise.resolve({ name });
    },
  };
  const run = (destination?: string, checkout = dir) =>
    model.methods.run.execute({
      session: "selected-session",
      checkout,
      destination,
      binary: "selected-yinz",
    }, c);
  async function rejects(action: () => Promise<unknown>) {
    let rejected = false;
    try {
      await action();
    } catch {
      rejected = true;
    }
    assert(rejected);
  }
  try {
    await Deno.writeTextFile(dir + "/Cargo.toml", "[workspace]\n");
    Deno.env.set("HOME", dir);
    Deno.Command = class extends original {
      constructor(command: string | URL, options: Deno.CommandOptions) {
        super(command, options);
        calls.push({ command, args: options.args! });
        assert(options.cwd === dir);
        this.output = () =>
          Promise.resolve({
            code: failure ? 1 : 0,
            success: !failure,
            signal: null,
            stdout: new TextEncoder().encode(
              options.args!.includes("new-pane")
                ? "terminal_42\n"
                : JSON.stringify(response),
            ),
            stderr: new TextEncoder().encode(failure ? "disconnected" : ""),
          });
      }
    };
    await run();
    assert(saved.get("config")?.destination === dir + "/.local/bin/yinz");
    const destination = dir + "/custom dir/$(touch INJECTED); 'yinz'";
    await run(destination);
    await run();
    assert(saved.get("config")?.destination === destination);
    assert(calls.length === 6);
    for (let i = 0; i < calls.length; i += 2) {
      assert(calls[i].command === "selected-yinz");
      assert(
        JSON.stringify(calls[i].args) === JSON.stringify([
          "--session",
          "selected-session",
          "action",
          "new-pane",
          "--name",
          `Install Yinz: ${dir}`,
          "--cwd",
          dir,
          "--",
          "cargo",
          "x",
          "install",
          i === 0 ? dir + "/.local/bin/yinz" : destination,
        ]),
      );
      assert(
        JSON.stringify(calls[i + 1].args) === JSON.stringify([
          "--session",
          "selected-session",
          "action",
          "pane-wait",
          "terminal_42",
          "--exited",
          "--timeout",
          "21600",
        ]),
      );
    }
    await run("~/expanded/yinz");
    assert(saved.get("config")?.destination === dir + "/expanded/yinz");
    for (const status of [12, null, undefined]) {
      response = {
        exited: true,
        ...(status === undefined ? {} : { exit_status: status }),
      };
      await rejects(() => run());
      assert(
        JSON.stringify(saved.get("result")?.response) ===
          JSON.stringify(response),
      );
    }
    const before = calls.length;
    for (const path of ["", "relative/yinz", "~someone/yinz", "bad\0path"]) {
      await rejects(() => run(path));
    }
    await rejects(() => run(undefined, "relative"));
    await rejects(() => run(undefined, dir + "/missing"));
    assert(calls.length === before);
    failure = true;
    await rejects(() => run());
    assert(calls.length === before + 1); // An uncertain dispatch is never retried.
  } finally {
    Deno.Command = original;
    if (home === undefined) Deno.env.delete("HOME");
    else Deno.env.set("HOME", home);
    await Deno.remove(dir, { recursive: true });
  }
});
