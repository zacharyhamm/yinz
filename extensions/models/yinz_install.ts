/** Install a selected local checkout in a visible Yinz pane. @module */
import { z } from "npm:zod@4.3.6";
import { checked, type Context, Literal } from "./_lib/workflow.ts";

const Path = Literal.refine(
  (s) => s.startsWith("/"),
  "Expected an absolute path",
);
const Args = z.object({
  session: Literal.refine((s) => s.length > 0, "Session is required"),
  checkout: Path,
  destination: Literal.nullish(),
  binary: Literal.default("yinz"),
});
const Config = z.object({ destination: Path });

/** One stable instance holds the install lock and remembered destination. */
export const model = {
  type: "@hammz/yinz-install",
  version: "2026.10.04.1",
  globalArguments: z.object({}),
  resources: {
    config: {
      description: "Remembered install destination for this Swamp repository",
      schema: Config,
      lifetime: "infinite",
      garbageCollection: 1,
    },
    pane: {
      description: "Created install pane and selected checkout",
      schema: z.object({
        session: z.string(),
        checkout: Path,
        destination: Path,
        pane: z.string(),
      }),
      lifetime: "infinite",
      garbageCollection: 10,
    },
    result: {
      description:
        "Actual pane-wait response, including failed or unknown status",
      schema: z.object({
        pane: z.string(),
        response: z.record(z.string(), z.unknown()),
      }),
      lifetime: "infinite",
      garbageCollection: 10,
    },
  },
  methods: {
    run: {
      description: "Run cargo x install and require a successful process exit",
      arguments: Args,
      execute: async (args: z.input<typeof Args>, c: Context) => {
        const a = Args.parse(args);
        if (!(await Deno.stat(a.checkout)).isDirectory) {
          throw new Error("Checkout must be a directory");
        }
        if (!(await Deno.stat(a.checkout + "/Cargo.toml")).isFile) {
          throw new Error("Checkout must contain Cargo.toml");
        }
        const saved = await c.readResource("config");
        let destination = a.destination ??
          (saved ? Config.parse(saved).destination : "~/.local/bin/yinz");
        if (destination.startsWith("~/")) {
          destination = Path.parse(Deno.env.get("HOME")) + destination.slice(1);
        }
        destination = Path.parse(destination);
        await Deno.mkdir(
          destination.slice(0, destination.lastIndexOf("/")) || "/",
          { recursive: true },
        );
        const config = await c.writeResource("config", "config", {
          destination,
        });
        const command = [a.binary, "--session", a.session, "action"];
        // One dispatch only: a lost reply can still mean a pane was created.
        const pane = await checked(c, a.checkout, [
          ...command,
          "new-pane",
          "--name",
          `Install Yinz: ${a.checkout}`,
          "--cwd",
          a.checkout,
          "--",
          "cargo",
          "x",
          "install",
          destination,
        ]);
        if (!/^terminal_\d+$/.test(pane)) {
          throw new Error(
            `Uncertain install dispatch: invalid pane ID ${
              JSON.stringify(pane)
            }; inspect session before retrying`,
          );
        }
        const handle = await c.writeResource("pane", "pane", {
          session: a.session,
          checkout: a.checkout,
          destination,
          pane,
        });
        const response = z.record(z.string(), z.unknown()).parse(
          JSON.parse(
            await checked(c, a.checkout, [
              ...command,
              "pane-wait",
              pane,
              "--exited",
              "--timeout",
              "21600",
            ]),
          ),
        );
        const result = await c.writeResource("result", "result", {
          pane,
          response,
        });
        if (response.exited !== true || response.exit_status !== 0) {
          throw new Error(
            `Install in ${pane} did not report success: ${
              JSON.stringify(response)
            }`,
          );
        }
        return { dataHandles: [config, handle, result] };
      },
    },
  },
};
