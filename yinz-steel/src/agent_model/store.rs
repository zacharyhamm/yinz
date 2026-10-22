use super::{decode, encode};
use agent_proto::store;
use anyhow::{bail, Result};
use serde_json::Value;
use yinz_types::{
    data::FetchRequest,
    internal_ids::{HostId, SessionName},
};

pub(super) fn run(op: &str, args: Value) -> Result<Value> {
    match op {
        "SharedDefs" => encode(store::shared_defs()),
        "FetchLimit" => encode(store::LIST_FETCH_LIMIT),
        "EntityRequest" => encode(FetchRequest::entity(decode::<String>(args)?)),
        "OwnedBy" => encode(store::owned_by(&owner(&args)?)),
        "OwnedRowsRequest" => encode(store::owned_rows_request(
            &decode::<String>(args["entity"].clone())?,
            &owner(&args)?,
        )),
        "ImportMarkerMutation" => encode(store::import_marker_mutation(
            &decode::<HostId>(args["host_id"].clone())?,
            &decode::<String>(args["what"].clone())?,
        )),
        _ => bail!("unknown store model operation {op:?}"),
    }
}
fn owner(args: &Value) -> Result<store::Owner> {
    Ok(store::Owner::new(
        decode::<HostId>(args["host_id"].clone())?,
        decode::<SessionName>(args["yinz_session_name"].clone())?,
    ))
}
