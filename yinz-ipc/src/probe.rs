use crate::ipc::{IpcRecv, ServerToClientMsg};
use std::io;
/// What a liveness probe found behind a session socket.
///
/// Three answers, not two, because "did not answer" and "nobody there" call
/// for opposite reactions: an absent server's name is free to reuse, while an
/// unresponsive one is still running and still owns its socket — creating a
/// session over it would unlink that socket and strand the live server with
/// no way for any client to reach it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionProbe {
    /// The server answered the probe; it can take a client.
    Alive,
    /// A server accepted the connection but did not answer within
    /// `PROBE_TIMEOUT` — it is running, but its main loop is not turning.
    Unresponsive,
    /// Nothing is listening on the socket.
    Absent,
    /// This process may not connect at all, as inside a sandbox that blocks Unix sockets.
    /// Whether a server is behind the socket is unknown, so it is kept like an unresponsive one.
    Denied,
}
/// The connect itself failed; this is what that means about the session.
///
/// The distinction that carries weight is timeout versus refusal. Nothing is
/// listening behind a refusal, so the socket is stale and its name is free. A
/// timeout is the opposite: a server whose listen backlog is full is *running*
/// — it is simply too far gone to accept anyone — and treating it as absent
/// would unlink the socket of a live process, stranding it where no client
/// could ever reach it again.
pub fn classify_probe_connect_error(kind: io::ErrorKind) -> Result<SessionProbe, io::ErrorKind> {
    match kind {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => Ok(SessionProbe::Absent),
        // `IPC_CONNECT_TIMEOUT` elapsed with the connection still unaccepted.
        // `WouldBlock` is the same condition reported under its other name: the
        // wait-mode docs name `TimedOut` in prose but link `WouldBlock`, and the
        // unix path returns `TimedOut`, so accept both rather than depend on it.
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Ok(SessionProbe::Unresponsive),
        // Usually a sandbox that blocks socket connections; otherwise a socket another user owns.
        io::ErrorKind::PermissionDenied => Ok(SessionProbe::Denied),
        kind => Err(kind),
    }
}
/// The connection was made; this is what came back over it.
///
/// Everything short of the expected answer counts as unresponsive rather than
/// absent: the connect succeeded, so something is there. A timeout arrives as
/// `IpcRecv::Idle`; a hangup mid-probe (`Eof`) or a garbage frame is a server
/// in some trouble, and the safe reading of trouble is "do not build over it".
pub fn classify_probe_reply(reply: IpcRecv<ServerToClientMsg>) -> SessionProbe {
    match reply {
        IpcRecv::Msg(ServerToClientMsg::Connected { .. }, _) => SessionProbe::Alive,
        IpcRecv::Msg(..) | IpcRecv::Idle | IpcRecv::Eof | IpcRecv::Malformed => {
            SessionProbe::Unresponsive
        },
    }
}
