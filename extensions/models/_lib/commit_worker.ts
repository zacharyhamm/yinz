/** The supervised pane owns Git and its locks; cancelling Swamp only stops observation. */
import { z } from "npm:zod@4.3.6";
import { CommitAndRebase, evidence, git, locked } from "./git_checkout.ts";
import { Literal } from "./workflow.ts";

const args = CommitAndRebase.extend({
    author: z.array(Literal),
    directory: Literal.min(1),
}).parse(JSON.parse(Deno.args[0]));
const c = { signal: new AbortController().signal };
// The server can carry the Git environment of a different checkout.
for (
    const name of (await git(c, args.source, ["rev-parse", "--local-env-vars"]))
        .split("\n")
) {
    Deno.env.delete(name);
}
const file = await Deno.open(`${args.directory}/output.log`, {
    create: true,
    write: true,
});
const log = file.writable.getWriter();
const screen = Deno.stdout.writable.getWriter();
const emit = async (bytes: Uint8Array) => {
    await log.write(bytes);
    await screen.write(bytes);
};
const print = (text: string) => emit(new TextEncoder().encode(text));
let exitCode = 125;
try {
    // ponytail: locks include dismissal; shorten only if queued mutations must proceed sooner.
    await locked(c, [args.source, args.target], async () => {
        let result;
        try {
            const before = await evidence(c, args);
            if (!before.target.clean || before.target.rebasing) {
                throw new Error("Target checkout is dirty or rebasing");
            }
            if (before.source.rebasing || before.source.conflicts.length) {
                throw new Error("Resolve the rebase before committing");
            }
            const child = new Deno.Command("bash", {
                args: [
                    "-c",
                    String.raw`
set -eu
exec 2>&1
target=$1 message=$2 dirty=$3
shift 3
run() {
    printf '$'
    printf ' %q' "$@"
    printf '\n'
    "$@"
}
if [ "$dirty" = true ]; then
    run git add -A
    run git "$@" commit -m "$message"
else
    printf 'No changes to commit.\n'
fi
run git rebase "$target"
`,
                    "commit-and-rebase",
                    before.target.commit,
                    args.message,
                    String(!before.source.clean),
                    ...args.author,
                ],
                cwd: before.source.checkout,
                env: {
                    GIT_TERMINAL_PROMPT: "0",
                    GIT_EDITOR: "true",
                    GIT_PAGER: "cat",
                },
                stdin: "inherit",
                stdout: "piped",
                stderr: "inherit",
            }).spawn();
            for await (const bytes of child.stdout) await emit(bytes);
            exitCode = (await child.status).code;
            result = { exitCode, evidence: await evidence(c, args) };
        } catch (error) {
            exitCode = 124;
            result = { exitCode, error: String(error) };
            await print(`${error}\n`);
        }
        await Deno.writeTextFile(
            `${args.directory}/result.json`,
            JSON.stringify(result),
        );
        await print(
            `\nCommit and rebase finished (exit ${exitCode}). Press Enter to return.\n`,
        );
        const byte = new Uint8Array(1);
        while (true) {
            if (await Deno.stdin.read(byte) === null) {
                exitCode = 125;
                break;
            }
            if (byte[0] === 10) break;
        }
    });
} finally {
    await log.close();
    screen.releaseLock();
}
Deno.exit(exitCode);
