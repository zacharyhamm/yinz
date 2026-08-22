use super::*;

mod errors;
mod formatting;
mod requests;

/// The arguments a tool call carries, built field by field. `Args` has no
/// builder and sixty-odd fields, so the tests go through `Args::parse` — which
/// is also what the real call path does, so a rename that breaks serde breaks
/// these too.
fn args(json: &str) -> Args {
    Args::parse(json).expect("test arguments parse")
}

/// The `query` document out of a request body.
fn document(body: &[u8]) -> String {
    let value: Value = serde_json::from_slice(body).expect("a request body is JSON");
    value["query"]
        .as_str()
        .expect("a query document")
        .to_owned()
}

/// The `variables` object out of a request body.
fn variables(body: &[u8]) -> Value {
    let value: Value = serde_json::from_slice(body).expect("a request body is JSON");
    value["variables"].clone()
}
