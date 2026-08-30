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

use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use hocus::ui;
use hocus::{Message, State};
use std::hint::black_box;

/// A typical hocus pane: wide enough for prose, narrow enough to wrap it.
const COLS: usize = 100;

/// Roughly what one 10 Hz frame's worth of tokens looks like.
const DELTA: &str = " and then the next few tokens arrive on the end of the reply,";

/// A reply with the markdown hocus actually renders — headings, prose, a list and a fenced block,
/// so the parser and the wrapper both do real work rather than walking a single paragraph.
fn reply(n: usize) -> String {
    format!(
        "## Finding {n}\n\nThe `render_transcript` path rewraps every message it is given, which \
         is fine until it happens ten times a second. Some detail follows, long enough that the \
         wrapper has to break it across several display lines.\n\n- it parses markdown\n- it wraps \
         each block to the pane width\n- it builds a style vector per character\n\n```rust\nfn \
         message_lines(message: &Message, cols: usize) -> Vec<VLine> {{\n    todo!(\"{n}\")\n}}\n\
         ```\n\nAnd a closing paragraph, because a reply rarely ends on a code fence."
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

criterion_group!(benches, bench);
criterion_main!(benches);
