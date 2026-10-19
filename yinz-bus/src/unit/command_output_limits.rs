use super::*;

#[tokio::test]
async fn command_capture_is_bounded_even_without_a_limit_and_still_streams_every_byte() {
    let cap = 8 * 1024 * 1024;
    let payload = vec![b'x'; cap + 8192];
    for requested in [None, Some(usize::MAX), Some(1024)] {
        let mut streamed = 0;
        let output = read_command_stream(payload.as_slice(), requested, |chunk| {
            assert!(chunk.iter().all(|byte| *byte == b'x'));
            streamed += chunk.len();
        })
        .await
        .unwrap();
        let limit = requested.unwrap_or(cap).min(cap);
        assert_eq!(output.len(), limit, "requested {requested:?}");
        assert!(output.ends_with(OUTPUT_TRUNCATED));
        assert!(output[..limit - OUTPUT_TRUNCATED.len()]
            .iter()
            .all(|byte| *byte == b'x'));
        assert_eq!(streamed, payload.len(), "the child must still be drained");
    }
}
