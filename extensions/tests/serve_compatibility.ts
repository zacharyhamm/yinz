/** Installed-binary gate: deno run --allow-all extensions/tests/serve_compatibility.ts
 * Uses a disposable local datastore. No Zellij session or production data is touched.
 */
const root = await Deno.makeTempDir({ prefix: "zellij-serve-compatibility-" });
const env = Deno.env.toObject();
for (const key of Object.keys(env)) {
    if (key.startsWith("SWAMP_")) delete env[key];
}
const binary = Deno.args[0] ?? "swamp";
const decoder = new TextDecoder();
let token = "";
let server: Deno.ChildProcess | undefined;
let drained: Promise<void> | undefined;
const sockets: WebSocket[] = [];
const redact = (value: string) =>
    token ? value.replaceAll(token, "[redacted]") : value;

async function bounded<T>(
    promise: Promise<T>,
    what: string,
    ms = 30_000,
): Promise<T> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
        return await Promise.race([
            promise,
            new Promise<never>((_, reject) => {
                timer = setTimeout(
                    () => reject(new Error(`Timed out: ${what}`)),
                    ms,
                );
            }),
        ]);
    } finally {
        clearTimeout(timer);
    }
}

async function cli(...args: string[]) {
    const child = new Deno.Command(binary, {
        args: [...args, "--json", "--no-telemetry"],
        cwd: root,
        env,
        clearEnv: true,
        stdout: "piped",
        stderr: "piped",
    }).spawn();
    let output;
    try {
        output = await bounded(
            child.output(),
            args.slice(0, 3).join(" "),
            60_000,
        );
    } catch (error) {
        try {
            child.kill("SIGKILL");
        } catch { /* Already exited. */ }
        await child.status;
        throw error;
    }
    if (!output.success) {
        // Token commands can emit credentials; never include their output in diagnostics.
        throw new Error(
            `swamp ${args.slice(0, 3).join(" ")} failed (${output.code})`,
        );
    }
    try {
        return JSON.parse(decoder.decode(output.stdout));
    } catch {
        throw new Error(
            `Invalid JSON from swamp ${args.slice(0, 3).join(" ")}`,
        );
    }
}

type Frame = {
    type: string;
    id: string;
    event?: { runId?: string; seq?: number };
    payload?: { runId?: string };
};

async function until(what: string, predicate: () => Promise<boolean>) {
    const deadline = Date.now() + 30_000;
    do {
        if (await predicate()) return;
        await new Promise((resolve) => setTimeout(resolve, 50));
    } while (Date.now() < deadline);
    throw new Error(`Timed out: ${what}`);
}

async function connect(url: string) {
    const ws = new WebSocket(url, [`bearer.${token}`]);
    sockets.push(ws);
    const messages = new ReadableStream<Frame>({
        start(controller) {
            ws.onmessage = (event) => {
                try {
                    controller.enqueue(JSON.parse(event.data));
                } catch (error) {
                    controller.error(error);
                }
            };
            ws.onclose = () => controller.close();
            ws.onerror = () => controller.error(new Error("WebSocket failed"));
        },
    }).getReader();
    await bounded(
        new Promise<void>((resolve, reject) => {
            ws.addEventListener("open", () => resolve(), { once: true });
            ws.addEventListener(
                "error",
                () => reject(new Error("WebSocket connection failed")),
                {
                    once: true,
                },
            );
        }),
        "WebSocket connection",
    );
    return {
        ws,
        async next() {
            const result = await bounded(messages.read(), "server response");
            if (result.done) throw new Error("Server closed the connection");
            return result.value;
        },
        send(type: string, id: string, payload: unknown) {
            ws.send(JSON.stringify({ type, id, payload }));
        },
    };
}

try {
    console.log(`Fixture: ${root}`);
    const help = await cli("help", "serve");
    console.log(`Swamp: ${help.version}`);
    await cli("repo", "init", root, "--tool", "none");
    await cli("vault", "create", "local_encryption", "zellij-probe");
    await cli(
        "access",
        "token",
        "mint",
        "zellij-probe",
        "--principal",
        "user:zellij-probe",
        "--vault",
        "zellij-probe",
        "--duration",
        "10m",
    );
    const revealed =
        (await cli("access", "token", "reveal", "zellij-probe", "--yes")).token;
    if (typeof revealed !== "string" || !revealed) {
        throw new Error("Token reveal returned no credential");
    }
    token = revealed;
    await cli("model", "create", "command/shell", "probe");
    await cli("workflow", "schema", "get");
    const workflow = await cli("workflow", "create", "zellij-detach-probe");
    await Deno.writeTextFile(
        workflow.path,
        JSON.stringify({
            id: workflow.id,
            name: workflow.name,
            version: 1,
            jobs: [{
                name: "main",
                steps: [{
                    name: "hold",
                    task: {
                        type: "model_method",
                        modelIdOrName: "probe",
                        methodName: "execute",
                        inputs: {
                            // A handshake holds execution across disconnect; elapsed time is not the assertion.
                            run: "touch started; while [ ! -f release ]; do sleep 0.05; done; echo completed",
                            workingDir: root,
                            timeout: 60_000,
                        },
                    },
                }, {
                    name: "approve",
                    dependsOn: [{
                        step: "hold",
                        condition: { type: "succeeded" },
                    }],
                    task: { type: "manual_approval", prompt: "Resume probe" },
                }, {
                    name: "inline-after-resume",
                    dependsOn: [{
                        step: "approve",
                        condition: { type: "succeeded" },
                    }],
                    task: {
                        type: "model_method",
                        modelType: "command/shell",
                        modelName: "inline-probe",
                        methodName: "execute",
                        inputs: { run: "echo resumed", workingDir: root },
                    },
                }],
            }],
        }),
    );
    await cli("workflow", "validate", workflow.name);
    server = new Deno.Command(binary, {
        args: [
            "serve",
            "--host",
            "127.0.0.1",
            "--port",
            "0",
            "--auth-mode",
            "token",
            "--admins",
            "user:zellij-probe",
            "--no-schedule",
            "--json",
            "--no-telemetry",
        ],
        cwd: root,
        env,
        clearEnv: true,
        stdout: "piped",
        stderr: "piped",
    }).spawn();
    // Drain stderr without retaining potentially sensitive server diagnostics.
    drained = server.stderr.pipeTo(new WritableStream({ write() {} }));
    const reader = server.stdout.pipeThrough(new TextDecoderStream())
        .getReader();
    const announcement = await bounded(
        (async () => {
            let buffered = "";
            for (;;) {
                const { value, done } = await reader.read();
                if (done) {
                    throw new Error(
                        "Server exited before announcing its endpoint",
                    );
                }
                buffered += value;
                let newline;
                while ((newline = buffered.indexOf("\n")) >= 0) {
                    const line = buffered.slice(0, newline);
                    buffered = buffered.slice(newline + 1);
                    try {
                        const parsed = JSON.parse(line);
                        if (parsed.status === "listening") return parsed;
                    } catch { /* Ignore non-protocol startup lines. */ }
                }
            }
        })(),
        "server startup",
    );
    console.log(`Startup: ${JSON.stringify(announcement)}`);
    const url = new URL(announcement.url);
    if (
        url.protocol !== "ws:" || url.hostname !== "127.0.0.1" ||
        !Number(url.port)
    ) {
        throw new Error(
            "Server did not announce an ephemeral loopback endpoint",
        );
    }
    await until("server readiness", async () => {
        const ready = await fetch(`http://${url.host}/ready`, {
            signal: AbortSignal.timeout(5_000),
        });
        await ready.body?.cancel();
        if (ready.status === 503) return false;
        if (!ready.ok) {
            throw new Error(`Server readiness failed: ${ready.status}`);
        }
        return true;
    });
    const first = await connect(url.href);
    first.send("workflow.run", "probe-run", {
        workflowIdOrName: workflow.name,
        noSupersede: true,
    });
    let runId: string | undefined;
    while (!runId) {
        const message = await first.next();
        console.log(`Run event: ${redact(JSON.stringify(message))}`);
        if (message.type === "error") throw new Error(JSON.stringify(message));
        runId = message.event?.runId ?? message.payload?.runId;
    }
    await until("workflow entered held step", async () => {
        try {
            await Deno.stat(`${root}/started`);
            return true;
        } catch (error) {
            if (!(error instanceof Deno.errors.NotFound)) throw error;
            return false;
        }
    });
    await bounded(
        new Promise<void>((resolve) => {
            first.ws.addEventListener("close", () => resolve(), { once: true });
            first.ws.close();
        }),
        "initial connection closed",
    );
    const second = await connect(url.href);
    second.send("run.attach", "probe-attach", { runId, afterSeq: 0 });
    let attached = false;
    while (!attached) {
        const message = await second.next();
        console.log(`Attach event: ${redact(JSON.stringify(message))}`);
        if (message.type === "error") {
            const history = await cli("workflow", "history", "get", runId);
            console.log(`History after disconnect: ${JSON.stringify(history)}`);
            await cli(
                "report",
                "get",
                "@swamp/workflow-summary",
                "--workflow",
                workflow.name,
            );
            throw new Error(
                "INCOMPATIBLE: cannot attach to the held run after disconnect",
            );
        }
        attached = message.type === "run.attached";
    }
    await Deno.writeTextFile(`${root}/release`, "");
    await until("detached workflow suspension", async () => {
        const history = await cli("workflow", "history", "get", runId!);
        if (history.status === "suspended") return true;
        if (["failed", "cancelled", "interrupted"].includes(history.status)) {
            await cli(
                "report",
                "get",
                "@swamp/workflow-summary",
                "--workflow",
                workflow.name,
            );
            throw new Error(`Detached run ended with ${history.status}`);
        }
        return false;
    });
    const resume = await connect(url.href);
    resume.send("workflow.approve", "approve", {
        workflowIdOrName: workflow.name,
        stepName: "approve",
        runId,
    });
    for (;;) {
        const frame = await resume.next();
        if (frame.type === "error") throw new Error(JSON.stringify(frame));
        if (frame.id === "approve") break;
    }
    resume.send("workflow.resume", "resume", {
        workflowIdOrName: workflow.name,
        runId,
        inputs: {},
    });
    await until("inline execution after resume", async () => {
        const history = await cli("workflow", "history", "get", runId!);
        if (history.status === "succeeded") return true;
        if (["failed", "cancelled", "interrupted"].includes(history.status)) {
            await cli(
                "report",
                "get",
                "@swamp/workflow-summary",
                "--workflow",
                workflow.name,
            );
            throw new Error(
                `INCOMPATIBLE: inline resume ended with ${history.status}`,
            );
        }
        return false;
    });
    console.log(
        "PASS: authenticated detached execution, attach/replay, suspension, and inline-type resume",
    );
} catch (error) {
    console.error(redact(String(error)));
    Deno.exitCode = 1;
} finally {
    await Deno.writeTextFile(`${root}/release`, "");
    for (const ws of sockets) {
        if (ws.readyState === WebSocket.OPEN) ws.close();
    }
    if (server) {
        try {
            server.kill("SIGTERM");
        } catch { /* Already exited. */ }
        try {
            await bounded(server.status, "server shutdown", 10_000);
        } catch {
            try {
                server.kill("SIGKILL");
            } catch { /* Already exited. */ }
            await server.status;
        }
        await drained;
    }
}
