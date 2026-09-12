//! The action dispatcher lives in `zellij_routing`; the per-client router thread that
//! calls it is `client_router`. Both keep their old paths here.
pub(crate) use crate::client_router::route_thread_main;
pub(crate) use zellij_bus::route::*;
pub(crate) use zellij_routing::*;
