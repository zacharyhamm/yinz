use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc,
};
use std::time::Duration;

struct PartialWrite {
    writes: Arc<AtomicUsize>,
    timeout: std::cell::Cell<Duration>,
    stalled: mpsc::Receiver<()>,
}
impl Read for PartialWrite {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}
impl Write for PartialWrite {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        // Timing is under test: a write can return partial progress at the end of its
        // socket timeout. The next write must not renew the frame's deadline.
        let _ = self.stalled.recv_timeout(self.timeout.get());
        Ok(1)
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("an incomplete frame must not flush")
    }
}
impl IpcStream for PartialWrite {
    fn try_clone_stream(&self) -> io::Result<Box<dyn IpcStream>> {
        Err(io::ErrorKind::Unsupported.into())
    }
    fn set_read_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        Ok(())
    }
    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.timeout.set(timeout.unwrap());
        Ok(())
    }
}

#[test]
fn partial_progress_cannot_renew_the_write_deadline_and_the_sender_is_invalidated() {
    for expired in [false, true] {
        let writes = Arc::new(AtomicUsize::new(0));
        let (_unblock, stalled) = mpsc::channel();
        let mut sender =
            IpcSenderWithContext::<ClientToServerMsg>::from_boxed(Box::new(PartialWrite {
                writes: writes.clone(),
                timeout: std::cell::Cell::new(Duration::ZERO),
                stalled,
            }));
        let deadline = Instant::now()
            + if expired {
                Duration::ZERO
            } else {
                Duration::from_millis(100)
            };
        let error = sender
            .send_client_msg_until(ClientToServerMsg::Paste("frame".into()), deadline)
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(writes.load(Ordering::SeqCst), usize::from(!expired));
        let error = sender
            .send_client_msg(ClientToServerMsg::ConnStatus)
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::BrokenPipe
        );
    }
}
