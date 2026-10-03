const MAX_CAPTURE_LEN: usize = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Ground,
    Esc,
    EscUnderscore,
    Capturing,
    CapturingEsc,
}

#[derive(Debug)]
pub struct KittyApcInterceptor {
    state: State,
    buffer: Vec<u8>,
    overflowed: bool,
}

pub struct ForwardBytes {
    bytes: [u8; 3],
    len: usize,
}

impl ForwardBytes {
    fn one(b0: u8) -> Self {
        ForwardBytes {
            bytes: [b0, 0, 0],
            len: 1,
        }
    }
    fn two(b0: u8, b1: u8) -> Self {
        ForwardBytes {
            bytes: [b0, b1, 0],
            len: 2,
        }
    }
    fn three(b0: u8, b1: u8, b2: u8) -> Self {
        ForwardBytes {
            bytes: [b0, b1, b2],
            len: 3,
        }
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

pub enum InterceptorResult {
    Forward(ForwardBytes),
    Swallow,
    /// `ESC _ G` is complete: flush preceding terminal bytes before capturing the payload.
    CaptureStarted,
    Captured(Vec<u8>),
}

/// A step of [`KittyApcInterceptor::split`]: what `advance` would have produced, with runs the
/// state machine would only have passed through borrowed from the input instead.
pub enum Piece<'a> {
    /// Terminal bytes up to the next `ESC _`, forwarded unchanged.
    Run(&'a [u8]),
    /// Terminal bytes the state machine held back and now returns.
    Forward(ForwardBytes),
    /// `ESC _ G` is complete: flush preceding terminal bytes before capturing the payload.
    CaptureStarted,
    Captured(Vec<u8>),
}

/// The interceptor driven over one read, in bulk where it can be. See
/// [`KittyApcInterceptor::split`].
pub struct Split<'i, 'a> {
    interceptor: &'i mut KittyApcInterceptor,
    bytes: &'a [u8],
    index: usize,
}

impl Split<'_, '_> {
    /// How much of the read the pieces so far consumed.
    pub fn position(&self) -> usize {
        self.index
    }
    /// Abandon the read: the caller requeues what is left, including any prefix it began.
    pub fn reset_interceptor(&mut self) {
        self.interceptor.reset();
    }
}

impl<'a> Iterator for Split<'_, 'a> {
    type Item = Piece<'a>;

    fn next(&mut self) -> Option<Piece<'a>> {
        while self.index < self.bytes.len() {
            if self.interceptor.is_capturing() {
                let run = self.interceptor.capture_run(&self.bytes[self.index..]);
                if run > 0 {
                    self.index += run;
                    continue;
                }
            }
            // Ordinary escapes return to ground after one or two bytes and are forwarded
            // unchanged, so the run extends to the next APC candidate. A final ESC stays with the
            // state machine, so a prefix split across reads still reaches it.
            if self.interceptor.is_in_ground_state() {
                let rest = &self.bytes[self.index..];
                let run = memchr::memmem::find(rest, b"\x1b_")
                    .unwrap_or_else(|| rest.len() - usize::from(rest.last() == Some(&0x1b)));
                if run > 0 {
                    self.index += run;
                    return Some(Piece::Run(&rest[..run]));
                }
            }
            let byte = self.bytes[self.index];
            self.index += 1;
            match self.interceptor.advance(byte) {
                InterceptorResult::Forward(bytes) => return Some(Piece::Forward(bytes)),
                InterceptorResult::Swallow => {},
                InterceptorResult::CaptureStarted => return Some(Piece::CaptureStarted),
                InterceptorResult::Captured(command) => return Some(Piece::Captured(command)),
            }
        }
        None
    }
}

impl KittyApcInterceptor {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        KittyApcInterceptor {
            state: State::Ground,
            buffer: Vec::new(),
            overflowed: false,
        }
    }
    pub fn reset(&mut self) {
        self.state = State::Ground;
        self.buffer.clear();
        self.overflowed = false;
    }
    /// Whether the interceptor is between sequences. In this state `advance` forwards every byte
    /// but `0x1b` unchanged and stays put, so a caller holding a whole pty read can forward the
    /// run up to the next escape in one go instead of driving the state machine byte by byte.
    pub fn is_in_ground_state(&self) -> bool {
        self.state == State::Ground
    }
    /// Whether the interceptor is inside a graphics payload, where `capture_run` applies.
    pub fn is_capturing(&self) -> bool {
        self.state == State::Capturing
    }
    /// Return an undecided `ESC` / `ESC _` prefix to the input queue when a host query pauses
    /// parsing before it. Replaying the prefix after the unconsumed bytes preserves stream order.
    pub fn take_pending_prefix(&mut self) -> &'static [u8] {
        let prefix: &[u8] = match self.state {
            State::Esc => b"\x1b",
            State::EscUnderscore => b"\x1b_",
            _ => return b"",
        };
        self.state = State::Ground;
        prefix
    }
    /// Capture payload bytes from the front of `bytes`, up to the first byte `advance` treats
    /// specially, and return how many were consumed. Equivalent to `advance` on each of them,
    /// which returns `Swallow` and stays in the payload, without a state-machine step per byte:
    /// a multi-megabyte direct upload spends most of its ingest here.
    pub fn capture_run(&mut self, bytes: &[u8]) -> usize {
        debug_assert!(self.is_capturing());
        let run = memchr::memchr3(0x1b, 0x9c, 0x18, bytes).unwrap_or(bytes.len());
        let run = memchr::memchr(0x1a, &bytes[..run]).unwrap_or(run);
        self.push_capture_bytes(&bytes[..run]);
        run
    }
    /// Drive the interceptor over `bytes`, producing what `advance` on each byte would, but
    /// with ground-state runs and payload runs taken in one step each. What both pane kinds
    /// ingest through.
    pub fn split<'i, 'a>(&'i mut self, bytes: &'a [u8]) -> Split<'i, 'a> {
        Split {
            interceptor: self,
            bytes,
            index: 0,
        }
    }
    pub fn advance(&mut self, byte: u8) -> InterceptorResult {
        match self.state {
            State::Ground => match byte {
                0x1b => {
                    self.state = State::Esc;
                    InterceptorResult::Swallow
                },
                b => InterceptorResult::Forward(ForwardBytes::one(b)),
            },
            State::Esc => match byte {
                0x5f => {
                    self.state = State::EscUnderscore;
                    InterceptorResult::Swallow
                },
                0x1b => InterceptorResult::Forward(ForwardBytes::one(0x1b)),
                b => {
                    self.state = State::Ground;
                    InterceptorResult::Forward(ForwardBytes::two(0x1b, b))
                },
            },
            State::EscUnderscore => match byte {
                0x47 => {
                    self.state = State::Capturing;
                    self.buffer.clear();
                    self.overflowed = false;
                    InterceptorResult::CaptureStarted
                },
                0x1b => {
                    self.state = State::Esc;
                    InterceptorResult::Forward(ForwardBytes::two(0x1b, 0x5f))
                },
                b => {
                    self.state = State::Ground;
                    InterceptorResult::Forward(ForwardBytes::three(0x1b, 0x5f, b))
                },
            },
            State::Capturing => self.advance_capturing(byte),
            State::CapturingEsc => match byte {
                0x5c => {
                    self.state = State::Ground;
                    if self.overflowed {
                        self.overflowed = false;
                        self.buffer.clear();
                        InterceptorResult::Swallow
                    } else {
                        InterceptorResult::Captured(std::mem::take(&mut self.buffer))
                    }
                },
                b => {
                    self.push_capture_byte(0x1b);
                    self.state = State::Capturing;
                    self.advance_capturing(b)
                },
            },
        }
    }
    fn advance_capturing(&mut self, byte: u8) -> InterceptorResult {
        match byte {
            0x1b => {
                self.state = State::CapturingEsc;
                InterceptorResult::Swallow
            },
            0x9c => {
                self.state = State::Ground;
                if self.overflowed {
                    self.overflowed = false;
                    self.buffer.clear();
                    InterceptorResult::Swallow
                } else {
                    InterceptorResult::Captured(std::mem::take(&mut self.buffer))
                }
            },
            0x18 | 0x1a => {
                self.state = State::Ground;
                self.buffer.clear();
                self.overflowed = false;
                InterceptorResult::Forward(ForwardBytes::one(byte))
            },
            b => {
                self.push_capture_byte(b);
                InterceptorResult::Swallow
            },
        }
    }
    /// `push_capture_byte` for each byte of `run`, which overflows on byte
    /// `MAX_CAPTURE_LEN + 1` and ignores everything after an overflow.
    fn push_capture_bytes(&mut self, run: &[u8]) {
        if self.overflowed || run.is_empty() {
            return;
        }
        if self.buffer.len() + run.len() > MAX_CAPTURE_LEN {
            self.overflowed = true;
            self.buffer.clear();
            return;
        }
        self.buffer.extend_from_slice(run);
    }
    fn push_capture_byte(&mut self, byte: u8) {
        if self.overflowed {
            return;
        }
        if self.buffer.len() == MAX_CAPTURE_LEN {
            self.overflowed = true;
            self.buffer.clear();
            return;
        }
        self.buffer.push(byte);
    }
}

#[cfg(test)]
#[path = "./unit/interceptor_tests.rs"]
mod interceptor_tests;
