/** Real lifecycle checks: deno run -A extensions/tests/managed_server.ts /absolute/zellij */
const binary = Deno.args[0];
if (!binary) throw new Error("Pass the built Zellij binary");
const root = await Deno.makeTempDir({ prefix: "zellij-managed-server-" });
const enc = new TextEncoder();
const dec = new TextDecoder();
const roots: string[] = [];
const sockets: WebSocket[] = [];
async function command(cwd: string, argv: string[], ok = true) {
    const result = await new Deno.Command(argv[0], {
        args: argv.slice(1),
        cwd,
        stdout: "piped",
        stderr: "piped",
    }).output();
    if (ok && !result.success) {
        throw new Error(
            `${argv.slice(0, 4).join(" ")} failed: ${
                dec.decode(result.stderr)
            }`,
        );
    }
    return { success: result.success, text: dec.decode(result.stdout) };
}
async function cli(cwd: string, ...args: string[]) {
    return JSON.parse(
        (await command(cwd, ["swamp", ...args, "--json", "--no-telemetry"]))
            .text,
    );
}
async function until(what: string, check: () => Promise<boolean>) {
    const deadline = Date.now() + 90_000;
    while (Date.now() < deadline) {
        if (await check()) return;
        await new Promise((resolve) => setTimeout(resolve, 50));
    }
    throw new Error(`Timed out: ${what}`);
}
async function statePath(cwd: string) {
    const bytes = new Uint8Array(
        await crypto.subtle.digest(
            "SHA-256",
            enc.encode(await Deno.realPath(cwd)),
        ),
    );
    return `/tmp/zellij-swamp-${Deno.uid()}/${
        Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")
    }/endpoint.json`;
}
async function state(cwd: string) {
    return JSON.parse(await Deno.readTextFile(await statePath(cwd)));
}
async function lifecycle(cwd: string, action: string, ok = true) {
    return await command(cwd, [binary, "workflow", "server", action], ok);
}
async function setup(name: string) {
    const cwd = `${root}/${name}`;
    await Deno.mkdir(cwd);
    roots.push(cwd);
    await command(cwd, ["git", "init", "-q"]);
    await cli(cwd, "repo", "init", cwd, "--tool", "none");
    return cwd;
}
async function connect(cwd: string) {
    const endpoint = await state(cwd);
    const ws = new WebSocket(endpoint.url, [`bearer.${endpoint.token}`]);
    sockets.push(ws);
    const queue: any[] = [];
    ws.onmessage = (e) => queue.push(JSON.parse(e.data));
    await new Promise<void>((resolve, reject) => {
        ws.onopen = () => resolve();
        ws.onerror = () => reject(new Error("connection failed"));
    });
    return async (type: string, payload: unknown, started = false) => {
        const id = crypto.randomUUID();
        ws.send(JSON.stringify({ id, type, payload }));
        let found: any;
        await until(type, async () => {
            found = queue.find((f) =>
                f.id === id &&
                (f.type === "error" ||
                    (started ? f.event?.runId : f.type !== "event"))
            );
            return !!found;
        });
        if (found.type === "error" || found.type === "run.interrupted") {
            throw new Error(
                JSON.stringify(found.error ?? { message: "interrupted" }),
            );
        }
        return started
            ? found.event.runId
            : found.payload?.data ?? found.payload;
    };
}
try {
    const failure = await setup("incompatible");
    const stub = `${root}/bin`;
    await Deno.mkdir(stub);
    await Deno.writeTextFile(
        `${stub}/swamp`,
        `#!/bin/sh\nprintf '%s\\n' '{"version":"20260909.131252.0-sha.375d7eca"}'\n`,
        { mode: 0o700 },
    );
    const rejected = await new Deno.Command(binary, {
        args: ["--swamp-server", failure],
        env: { PATH: `${stub}:${Deno.env.get("PATH")}` },
        stdout: "null",
        stderr: "piped",
    }).output();
    if (
        rejected.success ||
        !dec.decode(rejected.stderr).includes("inline model types")
    ) throw new Error("incompatible startup was not refused");
    const first = await setup("first");
    const second = await setup("second");
    const helpers = [1, 2].map(() =>
        new Deno.Command(binary, {
            args: ["--swamp-server", first],
            stdout: "null",
            stderr: "null",
        }).spawn()
    );
    await until("simultaneous startup", async () => {
        try {
            await state(first);
            return true;
        } catch {
            return false;
        }
    });
    const original = await state(first);
    const permissions = (await Deno.stat(await statePath(first))).mode!;
    if ((permissions & 0o077) !== 0) {
        throw new Error("credentials are not private");
    }
    const status = await lifecycle(first, "status");
    if (
        status.text.includes(original.token) ||
        status.text.includes(original.url)
    ) throw new Error("status exposed credentials");
    await lifecycle(second, "restart");
    if ((await state(second)).child_pid === original.child_pid) {
        throw new Error("checkouts share a server");
    }
    await until("one startup helper exits", async () =>
        await Promise.race([
            ...helpers.map(async (h) => {
                const s = await h.status;
                if (!s.success) throw new Error("startup contender failed");
                return true;
            }),
            new Promise<boolean>((r) => setTimeout(() => r(false), 100)),
        ]));
    const workflow = await cli(first, "workflow", "create", "lifecycle");
    await Deno.writeTextFile(
        workflow.path,
        JSON.stringify({
            id: workflow.id,
            name: workflow.name,
            version: 1,
            jobs: [{
                name: "main",
                steps: [
                    {
                        name: "hold",
                        task: {
                            type: "model_method",
                            modelType: "command/shell",
                            modelName: "hold",
                            methodName: "execute",
                            inputs: {
                                run: "touch started; while [ ! -f release ]; do sleep 0.05; done",
                                workingDir: first,
                                timeout: 120000,
                            },
                        },
                    },
                    {
                        name: "approve",
                        dependsOn: [{
                            step: "hold",
                            condition: { type: "succeeded" },
                        }],
                        task: { type: "manual_approval", prompt: "Paused" },
                    },
                ],
            }],
        }),
    );
    const request = await connect(first);
    const run = await request("workflow.run", {
        workflowIdOrName: workflow.name,
        noSupersede: true,
    }, true);
    for (const action of ["stop", "restart"]) {
        if ((await lifecycle(first, action, false)).success) {
            throw new Error(`${action} accepted active execution`);
        }
    }
    sockets[0].close();
    await Deno.writeTextFile(`${first}/release`, "");
    await until(
        "suspended after disconnect",
        async () =>
            (await cli(first, "workflow", "history", "get", run)).status ===
                "suspended",
    );
    await lifecycle(first, "stop");
    await lifecycle(first, "restart");
    const restarted = await state(first);
    if (restarted.child_pid === original.child_pid) {
        throw new Error("restart did not replace server");
    }
    if (
        (await cli(first, "workflow", "history", "get", run)).status !==
            "suspended"
    ) throw new Error("restart lost suspended run");
    await Deno.remove(`${first}/release`);
    await Deno.remove(`${first}/started`);
    const beforeCrash = await connect(first);
    const interrupted = await beforeCrash("workflow.run", {
        workflowIdOrName: workflow.name,
        noSupersede: true,
    }, true);
    await until("active step before crash", async () => {
        try {
            await Deno.stat(`${first}/started`);
            return true;
        } catch {
            return false;
        }
    });
    Deno.kill(restarted.child_pid, "SIGKILL");
    await until(
        "crash visible",
        async () => (await lifecycle(first, "status")).text.includes("exited"),
    );
    const helper = await command(
        first,
        [binary, "--swamp-server", first],
        false,
    );
    if (helper.success) throw new Error("crash silently launched replacement");
    await lifecycle(first, "restart");
    const recovered = await connect(first);
    let attached = true;
    try {
        await recovered("run.attach", { runId: interrupted, afterSeq: 0 });
    } catch {
        attached = false;
    }
    if (attached) throw new Error("crashed execution attached as active");
    const history = await cli(
        first,
        "workflow",
        "history",
        "search",
        "--workflow",
        workflow.name,
    );
    if (history.results.length !== 2) {
        throw new Error("recovery duplicated an execution");
    }
    const cancel = await recovered("workflow.run", {
        workflowIdOrName: workflow.name,
        noSupersede: true,
    }, true);
    const endpoint = await state(first);
    const response = await fetch(
        endpoint.url.replace("ws:", "http:") +
            `/api/v1/cancel/workflow-run/${cancel}`,
        {
            method: "POST",
            headers: { Authorization: `Bearer ${endpoint.token}` },
        },
    );
    if (!response.ok) throw new Error("active cancellation failed");
    await until(
        "active run cancellation",
        async () =>
            ["cancelled", "failed"].includes(
                (await cli(first, "workflow", "history", "get", cancel)).status,
            ),
    );
    if ((await state(first)).child_pid !== endpoint.child_pid) {
        throw new Error("cancellation replaced the shared server");
    }
    console.log(
        "PASS: startup race/failure, checkout isolation, private credentials, active stop/restart refusal, detached suspension, crash reconciliation without duplicates, and run-ID cancellation",
    );
} finally {
    for (const ws of sockets) if (ws.readyState === WebSocket.OPEN) ws.close();
    for (const cwd of roots) {
        await Deno.writeTextFile(`${cwd}/release`, "");
        try {
            await lifecycle(cwd, "stop", false);
        } catch { /* Retain failed fixtures for diagnosis. */ }
    }
    console.log(`Fixture: ${root}`);
}
