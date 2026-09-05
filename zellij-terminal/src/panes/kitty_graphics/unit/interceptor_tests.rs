use super::{InterceptorResult, KittyApcInterceptor};
#[derive(Default, Clone, PartialEq, Debug)]
struct RecordingPerformer {
    events: Vec<String>,
}

impl vte::Perform for RecordingPerformer {
    fn print(&mut self, c: char) {
        self.events.push(format!("print({:?})", c));
    }
    fn execute(&mut self, byte: u8) {
        self.events.push(format!("execute({})", byte));
    }
    fn hook(&mut self, params: &vte::Params, intermediates: &[u8], ignore: bool, action: char) {
        let params: Vec<Vec<u16>> = params.iter().map(|p| p.to_vec()).collect();
        self.events.push(format!(
            "hook({:?},{:?},{},{:?})",
            params, intermediates, ignore, action
        ));
    }
    fn put(&mut self, byte: u8) {
        self.events.push(format!("put({})", byte));
    }
    fn unhook(&mut self) {
        self.events.push("unhook".to_string());
    }
    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        let params: Vec<Vec<u8>> = params.iter().map(|p| p.to_vec()).collect();
        self.events
            .push(format!("osc({:?},{})", params, bell_terminated));
    }
    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        let params: Vec<Vec<u16>> = params.iter().map(|p| p.to_vec()).collect();
        self.events.push(format!(
            "csi({:?},{:?},{},{:?})",
            params, intermediates, ignore, action
        ));
    }
    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        self.events
            .push(format!("esc({:?},{},{})", intermediates, ignore, byte));
    }
}

/// Mirrors the ground-state batching in `TerminalPane::handle_pty_bytes`: while the interceptor
/// is between sequences it forwards every non-escape byte unchanged, so the whole run up to the
/// next escape is copied at once rather than driven through the state machine one byte at a time.
fn feed(
    interceptor: &mut KittyApcInterceptor,
    parser: &mut vte::Parser,
    performer: &mut RecordingPerformer,
    captured: &mut Vec<Vec<u8>>,
    input: &[u8],
) {
    let mut index = 0;
    while index < input.len() {
        if interceptor.is_in_ground_state() {
            let rest = &input[index..];
            let run_length = rest
                .iter()
                .position(|byte| *byte == 0x1b)
                .unwrap_or(rest.len());
            if run_length > 0 {
                parser.advance(performer, &rest[..run_length]);
                index += run_length;
                continue;
            }
        }
        let byte = input[index];
        index += 1;
        match interceptor.advance(byte) {
            InterceptorResult::Forward(fwd) => {
                for b in fwd.as_slice() {
                    parser.advance(performer, std::slice::from_ref(b));
                }
            },
            InterceptorResult::Swallow => {},
            InterceptorResult::Captured(cmd) => captured.push(cmd),
        }
    }
}

/// The pre-batching loop, kept as the reference the batched one is checked against.
fn feed_byte_at_a_time(
    interceptor: &mut KittyApcInterceptor,
    parser: &mut vte::Parser,
    performer: &mut RecordingPerformer,
    captured: &mut Vec<Vec<u8>>,
    input: &[u8],
) {
    for byte in input {
        match interceptor.advance(*byte) {
            InterceptorResult::Forward(fwd) => {
                for b in fwd.as_slice() {
                    parser.advance(performer, std::slice::from_ref(b));
                }
            },
            InterceptorResult::Swallow => {},
            InterceptorResult::Captured(cmd) => captured.push(cmd),
        }
    }
}

fn run_byte_at_a_time(input: &[u8], chunk_size: usize) -> (Vec<Vec<u8>>, RecordingPerformer) {
    let mut interceptor = KittyApcInterceptor::new();
    let mut parser = vte::Parser::new();
    let mut performer = RecordingPerformer::default();
    let mut captured = Vec::new();
    for chunk in input.chunks(chunk_size) {
        feed_byte_at_a_time(
            &mut interceptor,
            &mut parser,
            &mut performer,
            &mut captured,
            chunk,
        );
    }
    (captured, performer)
}

fn run_through_interceptor(input: &[u8], chunk_size: usize) -> (Vec<Vec<u8>>, RecordingPerformer) {
    let mut interceptor = KittyApcInterceptor::new();
    let mut parser = vte::Parser::new();
    let mut performer = RecordingPerformer::default();
    let mut captured = Vec::new();
    for chunk in input.chunks(chunk_size) {
        feed(
            &mut interceptor,
            &mut parser,
            &mut performer,
            &mut captured,
            chunk,
        );
    }
    (captured, performer)
}

fn run_bare(input: &[u8]) -> RecordingPerformer {
    let mut parser = vte::Parser::new();
    let mut performer = RecordingPerformer::default();
    for byte in input {
        parser.advance(&mut performer, std::slice::from_ref(byte));
    }
    performer
}

#[test]
fn split_invariance_across_arbitrary_chunk_sizes() {
    let corpus: &[u8] = b"hello \x1b[31mred\x1b[0m \x1b_Ga=T,f=24,s=1,v=1;AAAA\x1b\\ mid \x1bPq#0;2;0;0;0-\x1b\\ \x1b]0;title\x07 \x1b_Xnot-kitty\x1b\\ tail";
    let (whole_captured, whole_performer) = run_through_interceptor(corpus, corpus.len());
    for chunk_size in [1, 2, 3, 7, corpus.len()] {
        let (captured, performer) = run_through_interceptor(corpus, chunk_size);
        assert_eq!(captured, whole_captured, "chunk_size {}", chunk_size);
        assert_eq!(performer, whole_performer, "chunk_size {}", chunk_size);
    }
    assert_eq!(whole_captured, vec![b"a=T,f=24,s=1,v=1;AAAA".to_vec()]);
}

/// The corpus the batching is checked against: plain text, a kitty APC, sixel, OSC, CSI, and the
/// escape-byte edge cases the batch loop has to hand back to the state machine.
const MIXED_CORPUS: &[u8] = b"hello \x1b[31mred\x1b[0m \x1b_Ga=T,f=24,s=1,v=1;AAAA\x1b\\ mid \x1bPq#0;2;0;0;0-\x1b\\ \x1b]0;title\x07 \x1b_Xnot-kitty\x1b\\ \x1b\x1b[1mbold\x1b[0m tail\x1b";

#[test]
fn batched_ground_state_forwarding_matches_byte_at_a_time() {
    let inputs: Vec<&[u8]> = vec![
        MIXED_CORPUS,
        b"",
        b"\x1b",         // nothing but a lone escape
        b"\x1bhello",    // starts on an escape
        b"hello\x1b",    // ends on an escape
        b"\x1b\x1b\x1b", // back to back escapes
        b"plain text with no escapes at all",
        b"\x1b_Ga=q\x1b\\",               // nothing but a captured sequence
        b"a\x1b_Gb\x1b\\c\x1b_Gd\x1b\\e", // captures separated by single characters
    ];
    for input in inputs {
        for chunk_size in [1, 2, 3, 7, 64, usize::MAX] {
            let chunk_size = chunk_size.min(input.len().max(1));
            let batched = run_through_interceptor(input, chunk_size);
            let reference = run_byte_at_a_time(input, chunk_size);
            assert_eq!(
                batched, reference,
                "input {:?} at chunk size {}",
                input, chunk_size
            );
        }
    }
}

#[test]
fn is_in_ground_state_tracks_the_state_machine() {
    let mut interceptor = KittyApcInterceptor::new();
    assert!(
        interceptor.is_in_ground_state(),
        "a fresh interceptor is between sequences"
    );

    interceptor.advance(b'a');
    assert!(
        interceptor.is_in_ground_state(),
        "an ordinary byte does not move the state machine"
    );

    interceptor.advance(0x1b);
    assert!(!interceptor.is_in_ground_state(), "after ESC");
    interceptor.advance(b'_');
    assert!(!interceptor.is_in_ground_state(), "after ESC _");
    interceptor.advance(b'G');
    assert!(!interceptor.is_in_ground_state(), "capturing");
    interceptor.advance(b'x');
    assert!(!interceptor.is_in_ground_state(), "still capturing");
    interceptor.advance(0x1b);
    assert!(!interceptor.is_in_ground_state(), "capture ESC");
    interceptor.advance(b'\\');
    assert!(
        interceptor.is_in_ground_state(),
        "the sequence terminated, so we are between sequences again"
    );

    // an escape that turns out not to introduce an APC also returns to ground
    interceptor.advance(0x1b);
    interceptor.advance(b'[');
    assert!(interceptor.is_in_ground_state());

    interceptor.reset();
    assert!(interceptor.is_in_ground_state(), "after reset");
}

#[test]
fn an_interceptor_mid_sequence_does_not_batch_the_next_call() {
    // state persists across calls, so a buffer that arrives while a sequence is open must not be
    // treated as a run of ordinary bytes
    let mut interceptor = KittyApcInterceptor::new();
    interceptor.advance(0x1b);
    interceptor.advance(b'_');
    assert!(!interceptor.is_in_ground_state());

    let mut parser = vte::Parser::new();
    let mut performer = RecordingPerformer::default();
    let mut captured = Vec::new();
    feed(
        &mut interceptor,
        &mut parser,
        &mut performer,
        &mut captured,
        b"Gpayload\x1b\\after",
    );

    assert_eq!(captured, vec![b"payload".to_vec()]);
    assert_eq!(performer, run_bare(b"after"));
}

#[test]
fn kitty_apc_is_isolated_from_vte() {
    let (captured, performer) = run_through_interceptor(b"AB\x1b_Gq=2,a=d,d=A\x1b\\CD", usize::MAX);
    assert_eq!(performer, run_bare(b"ABCD"));
    assert_eq!(captured, vec![b"q=2,a=d,d=A".to_vec()]);
}

#[test]
fn non_kitty_streams_pass_through_byte_identically() {
    let inputs: Vec<&[u8]> = vec![
        b"\x1b_Xsome payload\x1b\\after",
        b"\x1bPq#0;2;0;0;0#0~~@@vv@@~~@@~~$#1!14@\x1b\\",
        "h\u{e9}llo\u{2192}".as_bytes(),
        b"\x1b]0;t\x07",
    ];
    for input in inputs {
        let (captured, performer) = run_through_interceptor(input, usize::MAX);
        assert_eq!(performer, run_bare(input), "input {:?}", input);
        assert!(captured.is_empty(), "input {:?}", input);
    }
}

#[test]
fn lone_trailing_esc_is_held_across_calls() {
    let mut interceptor = KittyApcInterceptor::new();
    let mut parser = vte::Parser::new();
    let mut performer = RecordingPerformer::default();
    let mut captured = Vec::new();
    feed(
        &mut interceptor,
        &mut parser,
        &mut performer,
        &mut captured,
        b"A\x1b",
    );
    feed(
        &mut interceptor,
        &mut parser,
        &mut performer,
        &mut captured,
        b"_Gx\x1b\\B",
    );
    assert_eq!(captured, vec![b"x".to_vec()]);
    assert_eq!(
        performer.events,
        vec!["print('A')".to_string(), "print('B')".to_string()]
    );
}

#[test]
fn esc_inside_payload_stays_in_capture_buffer() {
    let (captured, performer) = run_through_interceptor(b"\x1b_Ga\x1bb\x1b\\Z", usize::MAX);
    assert_eq!(captured, vec![b"a\x1bb".to_vec()]);
    assert_eq!(performer.events, vec!["print('Z')".to_string()]);
}

#[test]
fn oversized_sequence_is_discarded_and_stream_recovers() {
    let mut input = Vec::new();
    input.extend_from_slice(b"\x1b_G");
    input.extend(std::iter::repeat_n(b'j', 1_048_577));
    input.extend_from_slice(b"\x1b\\OK");
    let (captured, performer) = run_through_interceptor(&input, usize::MAX);
    assert!(captured.is_empty());
    assert_eq!(
        performer.events,
        vec!["print('O')".to_string(), "print('K')".to_string()]
    );
}

#[test]
fn can_and_sub_abort_capture_and_match_bare_vte() {
    for abort_byte in [0x18u8, 0x1au8] {
        let input = vec![0x1b, b'_', b'G', b'a', abort_byte, b'B'];
        let (captured, performer) = run_through_interceptor(&input, usize::MAX);
        assert!(captured.is_empty(), "abort_byte {}", abort_byte);
        assert_eq!(performer, run_bare(&input), "abort_byte {}", abort_byte);
    }
}

#[test]
fn c1_st_terminates_capture() {
    let (captured, performer) = run_through_interceptor(b"\x1b_Gx\x9cY", usize::MAX);
    assert_eq!(captured, vec![b"x".to_vec()]);
    assert_eq!(performer.events, vec!["print('Y')".to_string()]);
}

#[test]
fn esc_esc_underscore_sequences_replay_correctly() {
    let (captured, performer) = run_through_interceptor(b"\x1b\x1b_Gx\x1b\\A", usize::MAX);
    assert_eq!(captured, vec![b"x".to_vec()]);
    assert_eq!(performer, run_bare(b"\x1bA"));
}
