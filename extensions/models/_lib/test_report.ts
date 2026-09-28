/** Complete command capture and deterministic Cargo/nextest reports. @module */
import { z } from "npm:zod@4.3.6";

export const Capture = z.object({
    directory: z.string().regex(/^\/tmp\/yinz-test-[A-Za-z0-9_-]+$/),
});
export const ReportFields = z.object({
    summary: z.string().optional(),
    summaryLength: z.number().int().nonnegative().optional(),
    outputComplete: z.boolean().optional(),
    captureError: z.string().nullable().optional(),
    failedTests: z.array(z.string()).optional(),
});

export function captureCommand(
    directory: string,
    argv: string[],
    foreground = false,
    cancellation?: {
        binary: string;
        session: string;
        work: string;
        key: string;
    },
) {
    Capture.parse({ directory });
    const interrupt = cancellation
        ? `printf "%s\\n" "$cancel_request" | "$cancel_binary" --session "$cancel_session" pipe --name swamp --plugin yinz:swamp-bridge >/dev/null`
        : ":";
    return [
        "/bin/bash",
        "-c",
        // Literal argv remains positional. PIPESTATUS must be copied immediately;
        // tee failure must never replace the tested command's exit status.
        (foreground ? "unset $(git rev-parse --local-env-vars)\n" : "") +
        "dir=$1 cancel_binary=$2 cancel_session=$3 cancel_request=$4 finished_request=$5; shift 5\n" +
        `trap 'trap "" INT; touch "$dir/cancelled"; ${interrupt}' INT\n` +
        '(trap - INT; "$@" 2>&1 | tee "$dir/output" 2>"$dir/error"; codes=("${PIPESTATUS[@]}"); printf "%s %s\\n" "${codes[0]}" "${codes[1]}" >"$dir/status") <&0 & child=$!\n' +
        'while kill -0 "$child" 2>/dev/null; do wait "$child"; done\n' +
        'read -r code capture_code <"$dir/status" || code=125\n' +
        'if [ -e "$dir/cancelled" ]; then code=130; fi\n' +
        (cancellation
            ? 'printf \'%s,"process":%s}\\n\' "${finished_request%?}" "$$" | "$cancel_binary" --session "$cancel_session" pipe --name swamp --plugin yinz:swamp-bridge >/dev/null\n'
            : "") +
        (foreground
            ? 'trap \'exit "$code"\' INT\nprintf "\\nTests finished (exit %s). Press Enter to return.\\n" "$code"\nIFS= read -r dismissal || exit 125\n'
            : "") +
        'exit "$code"',
        "yinz-test-capture",
        directory,
        cancellation?.binary ?? "",
        cancellation?.session ?? "",
        JSON.stringify(
            cancellation
                ? {
                    method: "cancel_step",
                    work: cancellation.work,
                    operation: cancellation.key,
                }
                : {},
        ),
        JSON.stringify(
            cancellation
                ? {
                    method: "step_finished",
                    work: cancellation.work,
                    operation: cancellation.key,
                }
                : {},
        ),
        ...argv,
    ];
}

export async function readCapture(directory: string, exitCode: number | null) {
    Capture.parse({ directory });
    let output = "";
    const errors: string[] = [];
    try {
        output = await Deno.readTextFile(directory + "/output");
    } catch (error) {
        errors.push("Output unavailable: " + error);
    }
    try {
        const status = (await Deno.readTextFile(directory + "/status")).trim();
        const match = status.match(/^(\d+) (\d+)$/);
        if (!match || Number(match[1]) !== exitCode || match[2] !== "0") {
            errors.push("Capture status mismatch or tee failure: " + status);
        }
        const diagnostic = await Deno.readTextFile(directory + "/error");
        if (diagnostic) errors.push(diagnostic);
    } catch (error) {
        errors.push("Capture completion unavailable: " + error);
    }
    return {
        output,
        outputComplete: errors.length === 0,
        captureError: errors.length ? errors.join("\n") : null,
    };
}

export function testReport(test: {
    evidence: { gate: string[]; source: { commit: string } };
    outcome: string;
    exitCode: number | null;
    output: string;
    outputComplete?: boolean;
    captureError?: string | null;
}) {
    const output = test.output.replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, "")
        .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "").replace(/\r\n?/g, "\n");
    const failures = new Set<string>();
    let suite = "Cargo/libtest";
    let failureList = false;
    let recognized = false;
    let nextestOutput = false;
    for (const line of output.split("\n")) {
        const nextest = line.match(
            /^\s*(?:FAIL|TIMEOUT|ABORT|SIG[A-Z]+|LEAK|EXECFAIL)\s+\[[^\]]+\]\s+(?:\(\d+\/\d+\)\s+)?(.+?)\s*$/,
        );
        if (nextest) {
            failures.add(nextest[1]);
            recognized = true;
        }
        const running = line.match(
            /^\s*(?:Running (.+? \([^)]*\))|Doc-tests\s+(.+))\s*$/,
        );
        if (running) {
            suite = running[2] ? "doctest " + running[2] : running[1];
            nextestOutput = false;
        }
        if (/^\s*-{3,}\s+(?:STDOUT|STDERR):/.test(line)) nextestOutput = true;
        // nextest embeds libtest's own failure listing in each captured block.
        // Its binary-qualified status above is the identity for those failures.
        if (nextestOutput) continue;
        const status = line.match(/^test (.+?) \.\.\. FAILED\s*$/);
        const block = line.match(/^---- (.+?) (?:stdout|stderr) ----$/);
        if (status || block) {
            failures.add(suite + ": " + (status?.[1] ?? block![1]));
            recognized = true;
        }
        if (line === "failures:") {
            failureList = true;
            continue;
        }
        if (failureList && /^\s+\S/.test(line)) {
            failures.add(suite + ": " + line.trim());
        } else if (line.trim()) failureList = false;
        if (/test result:|Summary \[|Starting \d+ tests/.test(line)) {
            recognized = true;
        }
    }
    const failedTests = [...failures].sort();
    const passed = test.outcome === "completed" && test.exitCode === 0 &&
        test.outputComplete === true && failedTests.length === 0;
    const gateErrors = [];
    if (/^Diff in |>> Failed:.*format --check/m.test(output)) {
        gateErrors.push("formatting");
    }
    if (/>> Failed:.*lint|clippy::|error:.*lint/m.test(output)) {
        gateErrors.push("lint");
    }
    if (
        /^error: (?:could not compile|failed to run custom build command|failed to (?:get|download|select a version))/m
            .test(output)
    ) {
        gateErrors.push("build/command diagnostics (see below)");
    }
    const summary = [
        "Test report: " +
        (test.outcome === "cancelled"
            ? "CANCELLED"
            : passed
            ? "PASSED"
            : test.outputComplete !== true
            ? "INCOMPLETE"
            : "FAILED"),
        "Command (argv): " + JSON.stringify(test.evidence.gate),
        "Tested revision: " + test.evidence.source.commit,
        "Completion: " + test.outcome + "; exit status: " +
        (test.exitCode ?? "unavailable"),
        "Output: " +
        (test.outputComplete === true
            ? "complete"
            : "incomplete or unavailable"),
        ...(test.captureError ? ["Capture error: " + test.captureError] : []),
        "",
        ...(failedTests.length
            ? [
                "Failed tests (" + failedTests.length + "):",
                ...failedTests.map((name) => "- " + name),
            ]
            : [
                passed
                    ? "Gate succeeded. No failed tests reported."
                    : "No failed-test identities recognized; inspect diagnostics below.",
            ]),
        ...(gateErrors.length
            ? ["Non-test gate errors: " + gateErrors.join(", ")]
            : []),
        ...(!recognized
            ? [
                "Runner output was not recognized as Cargo/nextest; no test identities inferred.",
            ]
            : []),
        "",
        // Keep the complete diagnostic stream, including failure blocks and output
        // from unrecognized runners. A parser miss must never discard evidence.
        "Failure output and full diagnostics:",
        output ||
        (test.outputComplete
            ? "(command produced no output)"
            : "(output unavailable)"),
    ].join("\n");
    // CEL size() counts code points, while substring() uses UTF-16 offsets.
    return { summary, summaryLength: summary.length, failedTests, passed };
}
