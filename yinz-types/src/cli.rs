//! Typed, single-request administration through the selected session.
use crate::plugin_data::{FetchRequest, Mutation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DataRequest {
    Catalog {
        namespace: Option<String>,
    },
    Fetch(FetchRequest),
    Count(FetchRequest),
    Apply {
        namespace: String,
        mutations: Vec<Mutation>,
    },
}
