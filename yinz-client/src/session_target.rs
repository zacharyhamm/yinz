//! A session identity and its transport endpoint. Remote sockets never use the local namespace.
use std::path::PathBuf;
use yinz_types::internal_ids::SessionName;

#[derive(Debug, Clone)]
pub struct SessionTarget {
    pub name: SessionName,
    pub socket: PathBuf,
}

pub trait SessionEndpoint {
    fn name(&self) -> &SessionName;
    fn socket(&self) -> PathBuf;
}

impl SessionEndpoint for SessionName {
    fn name(&self) -> &SessionName {
        self
    }
    fn socket(&self) -> PathBuf {
        yinz_runtime::consts::YINZ_SOCK_DIR.join(self)
    }
}
impl SessionEndpoint for SessionTarget {
    fn name(&self) -> &SessionName {
        &self.name
    }
    fn socket(&self) -> PathBuf {
        self.socket.clone()
    }
}
