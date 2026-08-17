pub mod grid_state;
pub mod interceptor;
pub mod parser;
pub mod replies;
pub mod store;
pub use grid_state::*;
pub use interceptor::*;
pub use parser::*;
pub use replies::*;
pub use store::*;

/// Shared fixtures for the conformance suite. Deliberately outside the glob re-exports above so
/// its `KittyReply` cannot collide with the protocol types.
#[cfg(test)]
pub mod test_support;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KittyHostSupport {
    Supported,
    Unsupported,
    ProtocolDisabled,
}

impl KittyHostSupport {
    pub fn from_host_capability(supported: bool) -> Self {
        if supported {
            KittyHostSupport::Supported
        } else {
            KittyHostSupport::Unsupported
        }
    }
    pub fn protocol_is_enabled(&self) -> bool {
        *self != KittyHostSupport::ProtocolDisabled
    }
    pub fn host_supports_graphics(&self) -> bool {
        *self == KittyHostSupport::Supported
    }
}
