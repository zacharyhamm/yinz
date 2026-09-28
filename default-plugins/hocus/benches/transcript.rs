//! What a hocus frame costs while a reply is streaming.
//!
//! `transcript/streaming_frame` is the number this exists for. Wrapping the transcript — a full
//! markdown parse and a per-character style vector for every line — is by far the most expensive
//! thing a render does, and the render memo used to be keyed on the growing reply's length, so
//! every one of the ten frames a second redrew the *entire* conversation. The cost was
//! proportional to the whole chat and it queued ahead of the user's keystrokes on the plugin
//! thread, which is what made typing during a stream unusable.
//!
//! So the shape matters more than the absolute number: `full` is expected to grow with the
//! transcript, and `streaming_frame` is expected to stay flat as it does. A `streaming_frame` that
//! tracks `full` means the per-message cache has stopped working.
//!
//! Note this measures the native build. The plugin runs as wasm, so treat these as relative
//! figures for comparing one revision against another, not as real frame times.
//!
//! `agents_history` is the same question asked of the agents pane's history half, which draws
//! one run's timeline. `full` is what a history-cache miss cost before `agents::ParsedRun`: every
//! entry parsed and wrapped again, on every resize step and every time the run grew. `resize`
//! is what a resize step costs now. `grown` is everything the pane does with the run hocus sends
//! once it has one more entry - the JSON fingerprinted and compared with what is held, decoded,
//! the parse brought up to date and drawn - so the decode is counted rather than left out of the
//! comparison. `resent` is the same run sent again, which is one fingerprint and nothing else.
//!
//! `agents_history_huge_block` asks it of a run whose entries are each one enormous block - a
//! code fence, a paragraph, a table - which `long_run`'s many-block entries never exercise. The
//! timeline draws four rows of each, so `resize` is expected to cost about what it does on
//! `long_run`, not to grow with the size of the blocks.

use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use hocus::ui;
use hocus::{Message, State};
use hocus_proto::agents::{self, AgentRun, RunEntry, RunEntryKind};
use std::hint::black_box;

/// A typical hocus pane: wide enough for prose, narrow enough to wrap it.
const COLS: usize = 100;

/// Roughly what one 10 Hz frame's worth of tokens looks like.
const DELTA: &str = " and then the next few tokens arrive on the end of the reply,";

/// A reply with the markdown hocus actually renders — headings, prose, a list, a fenced block
/// and a table, so the parser and the wrapper both do real work rather than walking a single
/// paragraph. The table is here because it is the one block laid out at *wrap* time: it is
/// re-fitted on every rewrap, which is exactly what `streaming_frame` is watching for.
fn reply(n: usize) -> String {
    format!(
        "## Finding {n}\n\nThe `render_transcript` path rewraps every message it is given, which \
         is fine until it happens ten times a second. Some detail follows, long enough that the \
         wrapper has to break it across several display lines.\n\n- it parses markdown\n- it wraps \
         each block to the pane width\n- it builds a style vector per character\n\n```rust\nfn \
         message_lines(message: &Message, cols: usize) -> Vec<VLine> {{\n    todo!(\"{n}\")\n}}\n\
         ```\n\n| block | wrapped at | cost\n|---|---|---:|\n| paragraph | wrap time | one pass          |\n| table | wrap time | one pass per cell |\n\nAnd a closing paragraph, because a reply rarely ends on a code fence."
    )
}

/// `count` alternating turns, ending on an assistant reply — the message a stream grows.
fn conversation(count: usize) -> Vec<Message> {
    (0..count)
        .map(|n| match n % 2 {
            0 => Message::user(format!(
                "why is typing so slow while it is streaming, take {n}?"
            )),
            _ => Message::assistant(reply(n)),
        })
        .collect()
}

/// A state holding `count` messages, with its render memo already warm.
fn warmed(count: usize) -> State {
    let mut state = State::default();
    state.messages = conversation(count);
    // Ensure the last message is the one a reply would be growing.
    if state.messages.len() % 2 == 1 {
        state.messages.push(Message::assistant(reply(0)));
    }
    let _ = ui::cached_transcript(&mut state, COLS);
    state
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("transcript");

    for count in [10usize, 50, 200] {
        group.throughput(Throughput::Elements(count as u64));

        // The from-scratch cost: what every frame used to pay. Expected to grow with `count`.
        let messages = conversation(count);
        group.bench_with_input(BenchmarkId::new("full", count), &count, |b, _| {
            b.iter(|| black_box(ui::transcript(black_box(&messages), COLS)));
        });

        // One frame of a streaming reply: append a few tokens to the last message and redraw.
        // Expected to stay flat as `count` grows — only the last message should be rebuilt.
        group.bench_with_input(
            BenchmarkId::new("streaming_frame", count),
            &count,
            |b, _| {
                let mut state = warmed(count);
                let base = state
                    .messages
                    .last()
                    .expect("the conversation is not empty")
                    .content
                    .len();
                b.iter_custom(|iters| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iters {
                        // Untimed: put the reply back to a fixed length and re-warm, so every
                        // iteration measures the same frame rather than an ever-growing one.
                        state
                            .messages
                            .last_mut()
                            .expect("still not empty")
                            .content
                            .truncate(base);
                        let _ = ui::cached_transcript(&mut state, COLS);

                        let start = Instant::now();
                        state
                            .messages
                            .last_mut()
                            .expect("still not empty")
                            .content
                            .push_str(DELTA);
                        black_box(ui::cached_transcript(&mut state, COLS));
                        elapsed += start.elapsed();
                    }
                    elapsed
                });
            },
        );

        // One wheel notch on a warm, idle chat: scroll then redraw. Expected flat in `count`
        // — the memo is keyed on the messages, not on the viewport, so scrolling must never
        // re-wrap anything.
        group.bench_with_input(BenchmarkId::new("scroll_frame", count), &count, |b, _| {
            let mut state = warmed(count);
            b.iter(|| {
                black_box(state.scroll_up(3));
                black_box(ui::cached_transcript(&mut state, COLS));
            });
        });
    }

    group.finish();
}

/// A run shaped like the one that made the agents pane stall on resize: 140 entries and a
/// quarter of a megabyte of markdown, mostly assistant messages, with a few plans and results
/// long enough to be capped.
fn long_run() -> AgentRun {
    let entries = (0..140u64)
        .map(|n| {
            let (kind, repeat) = match n % 10 {
                0 => (RunEntryKind::Prompt, 1),
                4 => (RunEntryKind::Plan, 12),
                9 => (RunEntryKind::Result, 6),
                _ => (RunEntryKind::Message, 1),
            };
            RunEntry {
                revision: n + 1,
                event_id: None,
                kind,
                text: reply(n as usize).repeat(repeat),
                created_at_ms: Some(n as i64 * 60_000),
            }
        })
        .collect();
    AgentRun {
        id: "claude:bench".to_owned(),
        pane_id: 1,
        yinz_session_name: "bench".to_owned().into(),
        host_id: yinz_tile::prelude::HostId::default(),
        origin_run_id: None,
        previous_run_id: None,
        session_id: "bench".to_owned(),
        kind: "claude".to_owned(),
        cwd: None,
        host: None,
        worktree: None,
        started_at_ms: 0,
        ended_at_ms: None,
        outcome: agents::RunOutcome::Active,
        revision: 140,
        summarized_revision: 0,
        summary: None,
        entries,
        omitted_entries: 0,
    }
}

/// A run of a few entries of 100KB each, every one of them a single markdown block.
fn huge_block_run() -> AgentRun {
    let fence = format!(
        "```\n{}\n```",
        (0..5_000)
            .map(|n| format!("let line_{n} = {n};"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let paragraph = "word ".repeat(20_000);
    let table = std::iter::once("| id | what |\n|---|---|".to_owned())
        .chain((0..4_000).map(|n| format!("| {n} | {} |", "cell ".repeat(4))))
        .collect::<Vec<_>>()
        .join("\n");
    let mut run = long_run();
    run.entries = [fence, paragraph, table]
        .into_iter()
        .cycle()
        .take(9)
        .zip(1u64..)
        .map(|(text, revision)| RunEntry {
            revision,
            event_id: None,
            kind: RunEntryKind::Message,
            text,
            created_at_ms: Some(revision as i64 * 60_000),
        })
        .collect();
    run.revision = run.entries.len() as u64;
    run
}

fn agents_history_huge_block(c: &mut Criterion) {
    let mut group = c.benchmark_group("agents_history_huge_block");
    let run = huge_block_run();
    let bytes: usize = run.entries.iter().map(|entry| entry.text.len()).sum();
    group.throughput(Throughput::Bytes(bytes as u64));

    group.bench_function("full", |b| {
        b.iter(|| black_box(agents::run_history(black_box(&run), COLS, 0, 0)));
    });

    group.bench_function("resize", |b| {
        let mut parsed = agents::ParsedRun::new(&run);
        let mut width = COLS;
        b.iter(|| {
            width = if width == COLS { COLS - 1 } else { COLS };
            black_box(parsed.lines(width, 0, 0))
        });
    });

    group.finish();
}

fn agents_history(c: &mut Criterion) {
    let mut group = c.benchmark_group("agents_history");
    let run = long_run();
    let bytes: usize = run.entries.iter().map(|entry| entry.text.len()).sum();
    group.throughput(Throughput::Bytes(bytes as u64));

    // Every entry parsed and wrapped from scratch: the old cost of every miss.
    group.bench_function("full", |b| {
        b.iter(|| black_box(agents::run_history(black_box(&run), COLS, 0, 0)));
    });

    // One resize step: a width the parse has not been wrapped to yet.
    group.bench_function("resize", |b| {
        let mut parsed = agents::ParsedRun::new(&run);
        let mut width = COLS;
        b.iter(|| {
            width = if width == COLS { COLS - 1 } else { COLS };
            black_box(parsed.lines(width, 0, 0))
        });
    });

    // hocus sending the run again with one more entry on it, at an unchanged width: what the
    // agents pane's `AGENT_RUN_DETAIL` handler and its next history miss do with it.
    group.bench_function("grown", |b| {
        let mut grown = run.clone();
        grown.entries.push(RunEntry {
            revision: 1_000,
            event_id: None,
            kind: RunEntryKind::Message,
            text: reply(1_000),
            created_at_ms: None,
        });
        let held = agents::run_payload_fingerprint(&serde_json::to_string(&run).expect("encoded"));
        let payload = serde_json::to_string(&grown).expect("encoded");
        let mut parsed = agents::ParsedRun::new(&run);
        let _ = parsed.lines(COLS, 0, 0);
        b.iter_custom(|iters| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iters {
                // Untimed: back to the shorter run, so each iteration measures one new entry.
                parsed.update(&run);
                let _ = parsed.lines(COLS, 0, 0);
                let start = Instant::now();
                if black_box(held) != agents::run_payload_fingerprint(black_box(&payload)) {
                    let decoded: AgentRun = serde_json::from_str(&payload).expect("decoded");
                    parsed.update(&decoded);
                    black_box(parsed.lines(COLS, 0, 0));
                }
                elapsed += start.elapsed();
            }
            elapsed
        });
    });

    // The same run sent again, which the handler answers by comparing bytes.
    group.bench_function("resent", |b| {
        let payload = serde_json::to_string(&run).expect("encoded");
        let held = agents::run_payload_fingerprint(&payload);
        b.iter(|| black_box(held == agents::run_payload_fingerprint(black_box(&payload))));
    });

    group.finish();
}

criterion_group!(benches, bench, agents_history, agents_history_huge_block);
criterion_main!(benches);
