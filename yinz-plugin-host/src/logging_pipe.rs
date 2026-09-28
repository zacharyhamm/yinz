use std::{collections::VecDeque, io::Write};

use log::{debug, error};

use serde::{Deserialize, Serialize};

// 16kB log buffer
const YINZ_MAX_PIPE_BUFFER_SIZE: usize = 16_384;
#[derive(Debug, Serialize, Deserialize)]
pub struct LoggingPipe {
    buffer: VecDeque<u8>,
    plugin_name: String,
    plugin_id: yinz_types::internal_ids::PluginId,
    /// Every line handed to the log, in order. The log itself is a global sink the tests
    /// cannot read back, and a pipe that swallowed every plugin line would otherwise pass
    /// its own suite on `buffer` arithmetic alone.
    #[cfg(test)]
    #[serde(skip)]
    logged: std::cell::RefCell<Vec<String>>,
}

impl LoggingPipe {
    pub fn new(plugin_name: &str, plugin_id: crate::PluginId) -> LoggingPipe {
        LoggingPipe {
            buffer: VecDeque::new(),
            plugin_name: String::from(plugin_name),
            plugin_id,
            #[cfg(test)]
            logged: Default::default(),
        }
    }

    fn log_message(&self, message: &str) {
        debug!(
            "|{:<25.25}| {} [{:<10.15}] {}",
            self.plugin_name,
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S.%3f"),
            format!("id: {}", self.plugin_id),
            message
        );
        #[cfg(test)]
        self.logged.borrow_mut().push(message.to_owned());
    }
}

impl Write for LoggingPipe {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.buffer.len() + buf.len() > YINZ_MAX_PIPE_BUFFER_SIZE {
            let error_msg = "Plugin stderr exceeded its buffer limit. Split large writes and end \
                each log message with a newline.";
            error!("{}: {}", self.plugin_name, error_msg);
            self.buffer.clear();
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error_msg,
            ));
        }

        self.buffer.extend(buf);
        self.flush()?;

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // Newlines frame bytes, independently of UTF-8 validity. Decode each
        // complete line so one invalid byte cannot poison all later diagnostics.
        self.buffer.make_contiguous();
        let bytes = self.buffer.as_slices().0;
        let consumed = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1);
        let lines = String::from_utf8_lossy(&bytes[..consumed]);
        for line in lines.split_terminator('\n') {
            self.log_message(line);
        }
        self.buffer.drain(..consumed);
        Ok(())
    }
}

// Unit tests
#[cfg(test)]
mod logging_pipe_test {

    use super::*;

    #[test]
    fn malformed_stderr_does_not_hide_later_lines_or_a_split_utf8_tail() {
        let mut pipe = LoggingPipe::new("TestPipe", crate::PluginId::new(0));
        pipe.write_all(b"bad \xff\nhealthy\n\xf0\x9f").unwrap();
        assert_eq!(*pipe.logged.borrow(), ["bad \u{fffd}", "healthy"]);
        pipe.write_all(b"\x98\xb1\nnext\n").unwrap();
        assert_eq!(
            *pipe.logged.borrow(),
            ["bad \u{fffd}", "healthy", "😱", "next"]
        );
        assert!(pipe.buffer.is_empty());
    }

    #[test]
    fn plugin_stderr_reaches_configured_log_once() {
        const CHILD: &str = "YINZ_TEST_LOGGING_PIPE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // The logger and log paths are process-global, so initialize them in isolation.
            let temp = tempfile::tempdir().expect("create isolated log directory");
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "logging_pipe::logging_pipe_test::plugin_stderr_reaches_configured_log_once",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("TMPDIR", temp.path())
                .output()
                .expect("run logger check in child process");
            assert!(
                output.status.success(),
                "logger check failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        yinz_runtime::logging::configure_logger();
        let mut pipe = LoggingPipe::new("StderrTest", crate::PluginId::new(42));
        writeln!(pipe, "plugin stderr regression diagnostic").expect("write plugin stderr");
        log::logger().flush();

        let log = std::fs::read_to_string(&*yinz_runtime::consts::YINZ_TMP_LOG_FILE)
            .expect("read configured log");
        assert_eq!(
            log.matches("plugin stderr regression diagnostic").count(),
            1
        );
        let line = log
            .lines()
            .find(|line| line.contains("plugin stderr regression diagnostic"))
            .unwrap();
        assert!(line.contains("|StderrTest               |"), "{line}");
        assert!(line.contains("[id: 42    ]"), "{line}");
    }

    #[test]
    fn write_without_endl_does_not_consume_buffer_after_flush() {
        let mut pipe = LoggingPipe::new("TestPipe", crate::PluginId::new(0));

        let test_buffer = b"Testing write";

        pipe.write_all(test_buffer).expect("Err write");
        pipe.flush().expect("Err flush");

        assert_eq!(pipe.buffer.len(), test_buffer.len());
        assert!(
            pipe.logged.borrow().is_empty(),
            "an unterminated line was logged early"
        );
    }

    #[test]
    fn write_with_single_endl_at_the_end_consumes_whole_buffer_after_flush() {
        let mut pipe = LoggingPipe::new("TestPipe", crate::PluginId::new(0));

        let test_buffer = b"Testing write \n";

        pipe.write_all(test_buffer).expect("Err write");
        pipe.flush().expect("Err flush");

        assert_eq!(pipe.buffer.len(), 0);
        assert_eq!(*pipe.logged.borrow(), ["Testing write "]);
    }

    #[test]
    fn write_with_endl_in_the_middle_consumes_buffer_up_to_endl_after_flush() {
        let mut pipe = LoggingPipe::new("TestPipe", crate::PluginId::new(0));

        let test_buffer = b"Testing write \n";
        let test_buffer2: &[_] = b"And the rest";

        pipe.write_all(
            [
                test_buffer,
                test_buffer,
                test_buffer,
                test_buffer,
                test_buffer2,
            ]
            .concat()
            .as_slice(),
        )
        .expect("Err write");
        pipe.flush().expect("Err flush");

        // the *tail* survives, not merely the right number of bytes: a drain from the wrong
        // end keeps the head of a line already logged and throws the partial one away
        pipe.buffer.make_contiguous();
        assert_eq!(pipe.buffer.as_slices().0, test_buffer2);
        assert_eq!(*pipe.logged.borrow(), ["Testing write "; 4]);
    }

    #[test]
    fn write_with_many_endl_consumes_whole_buffer_after_flush() {
        let mut pipe = LoggingPipe::new("TestPipe", crate::PluginId::new(0));

        let test_buffer: &[_] = b"Testing write \n";

        pipe.write_all(
            [
                test_buffer,
                test_buffer,
                test_buffer,
                test_buffer,
                test_buffer,
            ]
            .concat()
            .as_slice(),
        )
        .expect("Err write");
        pipe.flush().expect("Err flush");

        assert_eq!(pipe.buffer.len(), 0);
        assert_eq!(*pipe.logged.borrow(), ["Testing write "; 5]);
    }

    #[test]
    fn write_with_incorrect_byte_boundary_does_not_crash() {
        let mut pipe = LoggingPipe::new("TestPipe", crate::PluginId::new(0));

        let test_buffer = "😱".as_bytes();

        // make sure it's not valid utf-8 string if we drop last symbol
        assert!(std::str::from_utf8(&test_buffer[..test_buffer.len() - 1]).is_err());

        pipe.write_all(&test_buffer[..test_buffer.len() - 1])
            .expect("Err write");
        pipe.flush().expect("Err flush");

        assert_eq!(pipe.buffer.len(), test_buffer.len() - 1);
        assert!(
            pipe.logged.borrow().is_empty(),
            "a split code point must wait for its last byte, not be logged as {:?}",
            pipe.logged.borrow()
        );

        // the reason the bytes were kept: once the last one arrives the character is whole,
        // and it is logged as itself when its line ends
        pipe.write_all(&test_buffer[test_buffer.len() - 1..])
            .expect("Err write");
        assert_eq!(pipe.buffer.len(), test_buffer.len());
        assert!(pipe.logged.borrow().is_empty(), "no newline yet");
        pipe.write_all(b"\n").expect("Err write");
        assert_eq!(pipe.buffer.len(), 0);
        assert_eq!(*pipe.logged.borrow(), ["😱"]);
    }

    /// A partial code point belongs to the next line and cannot delay a complete diagnostic.
    #[test]
    fn a_complete_line_is_logged_before_a_following_split_code_point() {
        let mut pipe = LoggingPipe::new("TestPipe", crate::PluginId::new(0));
        let scream = "😱".as_bytes();

        pipe.write_all(&[b"line\n", &scream[..3]].concat())
            .expect("Err write");
        assert_eq!(pipe.buffer.len(), 3);
        assert_eq!(*pipe.logged.borrow(), ["line"]);

        pipe.write_all(&[&scream[3..], b"\n"].concat())
            .expect("Err write");
        assert_eq!(pipe.buffer.len(), 0);
        assert_eq!(*pipe.logged.borrow(), ["line", "😱"]);
    }

    /// After a drain the ring buffer's head moves, so a later line can wrap around its end.
    /// `flush` reads only the front slice, which is the whole buffer only after
    /// `make_contiguous`; without it a wrapped line has no newline in view and is never logged.
    #[test]
    fn a_line_that_wraps_the_ring_buffer_is_still_logged_whole() {
        let mut pipe = LoggingPipe::new("TestPipe", crate::PluginId::new(0));
        pipe.buffer = VecDeque::with_capacity(16);
        let capacity = pipe.buffer.capacity();

        // the line is logged and drained, and the partial byte after it keeps the head 11
        // bytes in (an emptied deque resets its head, which would hide the wrap)
        pipe.write_all(b"0123456789\nX").expect("Err write");
        assert_eq!(*pipe.logged.borrow(), ["0123456789"]);
        assert_eq!(pipe.buffer.len(), 1);

        // appended without the flush `write` would do, so the layout is observable
        let second = b"abcdefgh\n";
        assert!(
            11 + 1 + second.len() > capacity,
            "the second line must wrap"
        );
        pipe.buffer.extend(second);
        assert!(
            !pipe.buffer.as_slices().1.is_empty(),
            "precondition: the second line wraps around the ring buffer"
        );

        pipe.flush().expect("Err flush");
        assert_eq!(pipe.buffer.len(), 0);
        assert_eq!(*pipe.logged.borrow(), ["0123456789", "Xabcdefgh"]);
    }

    #[test]
    fn write_with_many_endls_consumes_everything_after_flush() {
        let mut pipe = LoggingPipe::new("TestPipe", crate::PluginId::new(0));
        let test_buffer: &[_] = b"Testing write \n";

        pipe.write_all(
            [test_buffer, test_buffer, b"\n", b"\n", b"\n"]
                .concat()
                .as_slice(),
        )
        .expect("Err write");
        pipe.flush().expect("Err flush");

        assert_eq!(pipe.buffer.len(), 0);
        // each bare newline is a blank line in the plugin's output, and is logged as one
        assert_eq!(
            *pipe.logged.borrow(),
            ["Testing write ", "Testing write ", "", "", ""]
        );
    }

    // ---- property-based tests (hegel) ----

    use hegel::generators;

    /// A write that is either arbitrary bytes or something shaped like real plugin output: text
    /// with lone `\n`, `\r\n`, and multi-byte UTF-8 deliberately split across the write boundary,
    /// exercising line framing independently of UTF-8 validity.
    #[hegel::composite]
    fn any_write(tc: &hegel::TestCase) -> Vec<u8> {
        if tc.draw(generators::booleans()) {
            // capped only for runtime; the 16KiB cap is exercised by the write count below
            return tc.draw(generators::binary().max_size(1024));
        }
        let pieces = vec![
            "hello".as_bytes().to_vec(),
            b"\n".to_vec(),
            b"\r\n".to_vec(),
            b"\n\n\n".to_vec(),
            "".as_bytes().to_vec(),
            // a 4-byte codepoint, and each of its prefixes on its own
            "\u{1f631}".as_bytes().to_vec(),
            "\u{1f631}".as_bytes()[..1].to_vec(),
            "\u{1f631}".as_bytes()[..2].to_vec(),
            "\u{1f631}".as_bytes()[..3].to_vec(),
            "\u{1f631}".as_bytes()[3..].to_vec(),
            "é".as_bytes()[..1].to_vec(),
            "é".as_bytes()[1..].to_vec(),
            vec![0xff],
            vec![b'x'; 8192],
        ];
        let count = tc.draw(generators::integers::<usize>().max_value(6));
        let mut out = vec![];
        for _ in 0..count {
            out.extend_from_slice(&tc.draw(generators::sampled_from(pieces.clone())));
        }
        out
    }

    #[hegel::test]
    fn buffer_never_grows_past_the_cap_for_any_write_sequence(tc: hegel::TestCase) {
        let write_count = tc.draw(generators::integers::<usize>().max_value(12));
        let writes: Vec<Vec<u8>> = (0..write_count).map(|_| tc.draw(any_write())).collect();

        let mut pipe = LoggingPipe::new("TestPipe", crate::PluginId::new(0));
        for (index, write) in writes.iter().enumerate() {
            // an over-cap write is reported as an error - the only signal a plugin's
            // `write_all` gets that its line was dropped - and clears the buffer, so a stale
            // partial line is not prepended to the next one
            let over_cap = pipe.buffer.len() + write.len() > YINZ_MAX_PIPE_BUFFER_SIZE;
            let result = pipe.write(write);
            assert_eq!(
                result.is_err(),
                over_cap,
                "write {} of {:?} was {} but {}",
                index,
                writes.iter().map(|w| w.len()).collect::<Vec<_>>(),
                if over_cap {
                    "over the cap"
                } else {
                    "within the cap"
                },
                if result.is_err() {
                    "reported an error"
                } else {
                    "succeeded"
                }
            );
            if over_cap {
                assert!(
                    pipe.buffer.is_empty(),
                    "an over-cap write must clear the buffer, {} bytes remain",
                    pipe.buffer.len()
                );
            }
            pipe.flush().expect("flush must not fail");
            assert!(
                pipe.buffer.len() <= YINZ_MAX_PIPE_BUFFER_SIZE,
                "after write {} of {:?} the residual buffer is {} bytes, past the {} cap",
                index,
                writes.iter().map(|w| w.len()).collect::<Vec<_>>(),
                pipe.buffer.len(),
                YINZ_MAX_PIPE_BUFFER_SIZE
            );
        }
        // whatever is left is an unterminated tail: it must contain no newline, or a line was
        // logged without being drained
        pipe.buffer.make_contiguous();
        let residual: Vec<u8> = pipe.buffer.iter().copied().collect();
        assert!(
            !residual.contains(&b'\n'),
            "residual buffer {:?} still holds a complete line after {:?}",
            residual,
            writes.iter().map(|w| w.len()).collect::<Vec<_>>()
        );
        // every logged line is one line: it was split on '\n', which never survives into it
        for line in pipe.logged.borrow().iter() {
            assert!(!line.contains('\n'), "logged line {line:?} holds a newline");
        }
    }
}
