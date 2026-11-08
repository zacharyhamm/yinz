//! Chronological archive scopes shared by the browser and archive reader.
use crate::plugin_data::{FetchRequest, Pred};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Cursor {
    // Null timestamps sort before dated entries, then by revision (legacy archives).
    pub created_at_ms: Option<i64>,
    pub revision: u64,
}
impl Cursor {
    pub fn before(self) -> Pred {
        let same = Pred::lt("revision", self.revision as i64);
        match self.created_at_ms {
            Some(at) => Pred::lt("created_at_ms", at)
                .or(Pred::is_null("created_at_ms"))
                .or(Pred::eq("created_at_ms", at).and(same)),
            None => Pred::is_null("created_at_ms").and(same),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range {
    pub start: Option<Cursor>,
    pub end: Option<Cursor>,
    pub worker: Option<String>,
}
impl Range {
    pub fn request(&self, id: &str) -> FetchRequest {
        let mut request = FetchRequest::entity("agent_run_entry").filter(Pred::eq("run_id", id));
        if let Some(start) = self.start {
            request = request.filter(Pred::Not(Box::new(
                start
                    .before()
                    .or(Pred::eq("revision", start.revision as i64)),
            )));
        }
        if let Some(end) = self.end {
            request = request.filter(end.before());
        }
        request
    }
}
