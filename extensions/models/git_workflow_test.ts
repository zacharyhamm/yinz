import { Evidence, extension } from "./git_workflow.ts";
import { checked, Claim, claimed, Context } from "./_lib/workflow.ts";
const methods = extension.methods[0];
const owner = {
    host_id: "test-host",
    session: "test",
    incarnation: "test-incarnation",
    work: "test-work",
    agent: "worker",
};
const sync = {
    work: owner.work,
    key: "sync",
    request: "{}",
    incarnation: owner.incarnation,
    owner,
    outcome: "completed" as const,
    clean: true as const,
    vm: null,
};
const originalCommand = Deno.Command;
class Command extends originalCommand {
    constructor(command: string | URL, opts: Deno.CommandOptions) {
        super(command, opts);
        if (command === "zellij") {
            this.spawn = () => ({
                stdin: new WritableStream<Uint8Array>(),
                output: () => this.output(),
            } as Deno.ChildProcess);
            this.output = () =>
                Promise.resolve({
                    code: 0,
                    success: true,
                    signal: null,
                    stdout: new TextEncoder().encode(
                        JSON.stringify({ result: sync }),
                    ),
                    stderr: new Uint8Array(),
                });
        }
    }
}

function assert(value: unknown, message = "assertion failed"): asserts value {
    if (!value) throw new Error(message);
}
async function rejects(action: () => Promise<unknown>, pattern: RegExp) {
    try {
        await action();
    } catch (e) {
        assert(pattern.test(String(e)), String(e));
        return;
    }
    throw new Error("Expected refusal");
}
function context(repoDir: string, repoPath: string) {
    const saved = new Map<string, Record<string, unknown>>();
    const c: Context & {
        logger: {
            info(s: string): void;
            debug(s: string): void;
            warn(s: string): void;
            error(s: string): void;
        };
    } = {
        repoDir,
        globalArgs: {
            repoPath,
            authorName: "Test",
            authorEmail: "test@example.invalid",
        },
        signal: new AbortController().signal,
        logger: { info() {}, debug() {}, warn() {}, error() {} },
        readResource: (name) => Promise.resolve(saved.get(name) ?? null),
        writeResource: (spec, name, value) => {
            const schema = extension
                .resources[spec as keyof typeof extension.resources]?.schema;
            if (schema) schema.parse(value);
            saved.set(name, structuredClone(value));
            return Promise.resolve({ name });
        },
    };
    return { c, saved };
}
Deno.test("checkout status only reduces the menu for clean local branches with no commits beyond the target", async () => {
    const dir = await Deno.makeTempDir();
    try {
        const main = dir + "/main", source = dir + "/topic", sub = dir + "/sub";
        const { c, saved } = context(dir, source);
        const git = (cwd: string, args: string[]) =>
            checked(c, cwd, [
                "git",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "protocol.file.allow=always",
                ...args,
            ]);
        for (const repo of [main, sub]) {
            await Deno.mkdir(repo);
            await git(repo, ["init", "-b", "main"]);
            await Deno.writeTextFile(repo + "/file", "base\n");
            await Deno.writeTextFile(repo + "/.gitignore", "ignored\n");
            await git(repo, ["add", "."]);
            await git(repo, ["commit", "-m", "base"]);
        }
        await git(main, ["submodule", "add", sub, "module"]);
        await git(main, ["commit", "-am", "submodule"]);
        await git(main, ["worktree", "add", "-b", "topic", source]);
        await git(source, ["submodule", "update", "--init"]);
        await git(source, ["config", "submodule.module.ignore", "all"]);
        const index = await git(source, [
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "index",
        ]);
        const inspect = async (
            label: string,
            unchanged: boolean,
            overrides: Record<string, string> = {},
            failed = false,
        ) => {
            const before = await Deno.readFile(index);
            await methods.checkout_status.execute(
                methods.checkout_status.arguments.parse({
                    source,
                    ...overrides,
                }),
                c,
            );
            const result = saved.get("checkoutStatus-current")!;
            assert(
                result.unchanged === unchanged,
                label + ": " + JSON.stringify(result),
            );
            assert(
                (result.error !== null) === failed,
                label + ": " + JSON.stringify(result),
            );
            assert(
                before.toString() === (await Deno.readFile(index)).toString(),
                "inspection rewrote the index",
            );
        };
        await inspect("equal to main", true);
        await Deno.writeTextFile(source + "/ignored", "ignored change");
        await inspect("ignored files do not count", true);
        await git(main, ["commit", "--allow-empty", "-m", "advance target"]);
        await inspect("behind main", true);
        await Deno.writeTextFile(source + "/file", "changed\n");
        await inspect("unstaged", false);
        await git(source, ["add", "file"]);
        await inspect("staged", false);
        await git(source, ["reset", "--hard", "HEAD"]);
        await Deno.writeTextFile(source + "/untracked", "new file");
        await inspect("untracked", false);
        await Deno.remove(source + "/untracked");
        const module = source + "/module";
        await Deno.writeTextFile(module + "/file", "changed submodule\n");
        await inspect("submodule unstaged despite ignore config", false);
        await git(module, ["add", "file"]);
        await inspect("submodule staged", false);
        await git(module, ["reset", "--hard", "HEAD"]);
        await Deno.writeTextFile(module + "/untracked", "new submodule file");
        await inspect("submodule untracked", false);
        await Deno.remove(module + "/untracked");
        await git(module, ["commit", "--allow-empty", "-m", "submodule ahead"]);
        await inspect("submodule commit", false);
        await git(module, ["reset", "--hard", "HEAD~"]);
        await inspect("submodule restored", true);
        for (const name of ["rebase-merge", "rebase-apply"]) {
            const path = await git(source, [
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                name,
            ]);
            await Deno.mkdir(path);
            await inspect("clean paused " + name, false);
            await Deno.remove(path);
        }
        await git(source, [
            "commit",
            "--allow-empty",
            "-m",
            "ahead without a diff",
        ]);
        await inspect("commits beyond main", false);
        await git(source, ["branch", "release"]);
        await inspect("configured local target", true, {
            targetBranch: "release",
        });
        await git(source, ["tag", "missing"]);
        await inspect("missing branch even with matching tag", false, {
            targetBranch: "missing",
        }, true);
        await inspect(
            "missing checkout",
            false,
            { source: dir + "/missing" },
            true,
        );
        await inspect("not a repository", false, { source: dir }, true);
        await Deno.mkdir(source + "/nested");
        await inspect("not a checkout root", false, {
            source: source + "/nested",
        }, true);
        await Deno.mkdir(dir + "/unborn");
        await git(dir + "/unborn", ["init", "-b", "main"]);
        await inspect("missing HEAD", false, { source: dir + "/unborn" }, true);
        const previousIndex = Deno.env.get("GIT_INDEX_FILE");
        try {
            Deno.env.set("GIT_INDEX_FILE", index);
            await inspect("inherited Git context", false, {}, true);
        } finally {
            if (previousIndex === undefined) Deno.env.delete("GIT_INDEX_FILE");
            else Deno.env.set("GIT_INDEX_FILE", previousIndex);
        }
        Deno.Command = class extends originalCommand {
            override output(): Promise<Deno.CommandOutput> {
                throw new Error("Injected Git inspection failure");
            }
        };
        await inspect("Git command error", false, {}, true);
    } finally {
        Deno.Command = originalCommand;
        await Deno.remove(dir, { recursive: true });
    }
});
Deno.test("guarded Git extension rejects dirty/stale evidence, rebases, integrates once, and refuses unsafe cleanup", async () => {
    Deno.Command = Command;
    const dir = await Deno.makeTempDir();
    try {
        const main = dir + "/main", source = dir + "/topic";
        await Deno.mkdir(main);
        const { c, saved } = context(dir, source);
        const git = (cwd: string, args: string[]) =>
            checked(c, cwd, [
                "git",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                ...args,
            ]);
        await git(main, ["init", "-b", "main"]);
        await Deno.writeTextFile(main + "/file", "base\n");
        await git(main, ["add", "."]);
        await git(main, ["commit", "-m", "base"]);
        await git(main, ["worktree", "add", "-b", "topic", source]);
        // Lua's saved null may be omitted; live inspection returns an explicit null.
        const args = {
            source,
            target: main,
            targetBranch: "main",
            gate: ["cargo", "x", "ci", "check"],
            vmSync: { ...sync, vm: undefined },
            binary: "zellij",
        };
        await Deno.writeTextFile(source + "/file", "topic\n");
        await methods.verify_checkout.execute(args, c);
        assert(!Evidence.parse(saved.get("evidence-current")).source.clean);
        await rejects(
            () => methods.rebase.execute({ ...args, key: "dirty" }, c),
            /dirty/,
        );
        const beforeDiff = await git(source, ["status", "--porcelain=v1"]);
        const untracked = 'untracked contents 😀é\t"\\\n'.repeat(40000);
        assert(new TextEncoder().encode(untracked).length > 1_100_000);
        await Deno.writeTextFile(source + "/untracked.txt", untracked);
        await methods.show_diff.execute(args, c);
        const output = String(saved.get("diff-current")?.output);
        assert(
            output.includes("Committed branch changes:") &&
                output.includes("Staged changes:") &&
                output.includes("Unstaged changes:"),
        );
        assert(
            output.includes("+topic") && output.includes("untracked contents"),
        );
        assert(
            output.endsWith(untracked),
            "the saved diff retains every character",
        );
        await Deno.remove(source + "/untracked.txt");
        assert(await git(source, ["status", "--porcelain=v1"]) === beforeDiff);
        await methods.guarded_commit.execute({
            ...args,
            key: "commit",
            message: "topic",
        }, c);
        const committed = await git(source, ["rev-parse", "HEAD"]);
        await methods.guarded_commit.execute({
            ...args,
            key: "empty-commit",
            message: "skip",
        }, c);
        assert(await git(source, ["rev-parse", "HEAD"]) === committed);
        await methods.rebase.execute({ ...args, key: "rebase" }, c);
        await methods.verify_checkout.execute(args, c);
        const proof = Evidence.parse(saved.get("evidence-current"));
        const input = {
            ...args,
            key: "integrate",
            approved: true as const,
            evidence: proof,
            review: {
                evidence: proof,
                outcome: "completed" as const,
                findings: [],
                reviewer: "separate-agent",
                valid: true as const,
                resolved: true as const,
            },
            test: {
                evidence: proof,
                outcome: "completed" as const,
                exitCode: 0 as const,
                log: "terminal_9",
                passed: true as const,
            },
        };
        await Deno.writeTextFile(source + "/untracked", "dirty");
        await methods.integrate.execute({ ...input, key: "dirty-evidence" }, c);
        assert(saved.get("integrationDecision-current")?.integrated === false);
        await Deno.remove(source + "/untracked");
        await git(main, ["commit", "--allow-empty", "-m", "target moved"]);
        await methods.integrate.execute({ ...input, key: "moved-target" }, c);
        assert(saved.get("integrationDecision-current")?.integrated === false);
        await methods.rebase.execute({ ...args, key: "fresh-rebase" }, c);
        await methods.verify_checkout.execute(args, c);
        const fresh = Evidence.parse(saved.get("evidence-current"));
        const ready = {
            ...input,
            evidence: fresh,
            review: { ...input.review, evidence: fresh },
            test: { ...input.test, evidence: fresh },
        };
        await methods.integrate.execute(ready, c);
        assert(await git(main, ["rev-parse", "HEAD"]) === fresh.source.commit);
        await methods.integrate.execute(ready, c); // identical dispatch retrieves its recorded result
        await rejects(
            () => methods.integrate.execute({ ...ready, gate: ["false"] }, c),
            /Conflicting request/,
        );
        // Local checkout evidence requires no worker/VM operation. Failed and skipped
        // checks can be accepted explicitly, but never replace revision freshness.
        Deno.Command = originalCommand;
        const local = { ...args, vmSync: { mode: "local" as const } };
        await methods.verify_checkout.execute(local, c);
        const localProof = Evidence.parse(saved.get("evidence-current"));
        for (
            const checks of [
                { review: null, test: null },
                {
                    review: {
                        ...input.review,
                        evidence: localProof,
                        resolved: false,
                        findings: [{
                            severity: "error" as const,
                            file: "file",
                            message: "Unresolved finding",
                        }],
                    },
                    test: {
                        ...input.test,
                        evidence: localProof,
                        outcome: "failed",
                        exitCode: 1,
                        passed: false,
                    },
                },
            ]
        ) {
            const informed = {
                ...local,
                key: checks.test ? "failed-checks" : "skipped-checks",
                evidence: localProof,
                ...checks,
                approved: true as const,
            };
            await rejects(
                () =>
                    methods.integrate.execute(
                        { ...informed, approved: false } as never,
                        c,
                    ),
                /true/,
            );
            await methods.integrate.execute(informed, c);
            assert(
                saved.get("integrationDecision-current")?.integrated === true,
            );
        }
        await Deno.writeTextFile(
            source + "/late-untracked",
            "changed after approval",
        );
        await methods.integrate.execute({
            ...local,
            key: "stale-skipped",
            evidence: localProof,
            review: null,
            test: null,
            approved: true,
        }, c);
        assert(saved.get("integrationDecision-current")?.integrated === false);
        await Deno.remove(source + "/late-untracked");
        Deno.Command = Command;
        await Deno.writeTextFile(source + "/keep", "important");
        await rejects(
            () =>
                methods.guarded_cleanup.execute({
                    ...args,
                    key: "refused-cleanup",
                    approved: true,
                    commit: fresh.source.commit,
                }, c),
            /changed/,
        );
        assert(await Deno.readTextFile(source + "/keep") === "important");
        await Deno.remove(source + "/keep");
        await methods.guarded_cleanup.execute({
            ...args,
            key: "cleanup",
            approved: true,
            commit: fresh.source.commit,
        }, c);
        await rejects(() => Deno.stat(source), /NotFound/);
    } finally {
        Deno.Command = originalCommand;
        await Deno.remove(dir, { recursive: true });
    }
});
Deno.test("rebase conflicts are inspected and abort is explicit; unknown dispatches are never repeated", async () => {
    Deno.Command = Command;
    const dir = await Deno.makeTempDir();
    try {
        const main = dir + "/main", source = dir + "/topic";
        await Deno.mkdir(main);
        const { c, saved } = context(dir, source);
        const git = (cwd: string, args: string[]) =>
            checked(c, cwd, [
                "git",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                ...args,
            ]);
        await git(main, ["init", "-b", "main"]);
        await Deno.writeTextFile(main + "/file", "base\n");
        await git(main, ["add", "."]);
        await git(main, ["commit", "-m", "base"]);
        await git(main, ["worktree", "add", "-b", "topic", source]);
        const args = {
            source,
            target: main,
            targetBranch: "main",
            gate: ["true"],
            vmSync: sync,
            binary: "zellij",
        };
        await Deno.writeTextFile(source + "/file", "topic\n");
        await methods.guarded_commit.execute({
            ...args,
            key: "commit",
            message: "topic",
        }, c);
        await Deno.writeTextFile(main + "/file", "target\n");
        await git(main, ["commit", "-am", "target"]);
        await methods.rebase.execute({ ...args, key: "conflict" }, c);
        const result = saved.get("rebaseResult-current")!;
        assert(result.exitCode !== 0);
        assert(
            Evidence.parse(result.evidence).source.conflicts.includes("file"),
        );
        await methods.rebase_abort.execute({ ...args, key: "abort" }, c);
        assert(await Deno.readTextFile(source + "/file") === "topic\n");
        let dispatched = 0;
        await rejects(() =>
            claimed(c, "lost", {}, () => {
                dispatched++;
                throw new Error("killed");
            }), /killed/);
        assert(Claim.parse(saved.get("claim-lost")).outcome === "unknown");
        await rejects(() =>
            claimed(c, "lost", {}, () => {
                dispatched++;
                return Promise.resolve({});
            }), /unknown/);
        assert(dispatched === 1);
    } finally {
        Deno.Command = originalCommand;
        await Deno.remove(dir, { recursive: true });
    }
});
