//! Requests always travel to the acting client, never to the plugin's map owner.
use crate::{
    plugin_map::PluginEnv,
    yinz_exports::{session_list_snapshot, write_response},
};
use anyhow::ensure;
use prost::Message;
use std::sync::mpsc;
use yinz_bus::server_instruction::ServerInstruction;
use yinz_runtime::errors::prelude::*;
use yinz_types::{data::ClientId, session_destination::*};

fn scoped_id(env: &PluginEnv, id: &str) -> Result<String> {
    ensure!(
        !id.is_empty() && id.len() <= 128,
        "Invalid session request ID"
    );
    Ok(format!(
        "{}:{}:{id}",
        env.plugin_id,
        env.generation.unwrap_or_default()
    ))
}

fn enqueue(
    env: &PluginEnv,
    client: ClientId,
    request: ClientRequest,
) -> Result<mpsc::Receiver<Result<ClientReply, String>>> {
    let (reply, receiver) = mpsc::channel();
    env.senders
        .send_to_server(ServerInstruction::SessionRequest {
            client,
            request: Request {
                token: uuid::Uuid::new_v4().to_string(),
                request,
            },
            reply,
        })
        .context("request client session operation")?;
    Ok(receiver)
}

fn request(env: &mut PluginEnv, request: ClientRequest) -> Result<ClientReply> {
    let new_preparation = if let ClientRequest::Switch(SwitchRequest::Prepare { id, .. }) = &request
    {
        let key = (env.client_id, id.clone());
        env.session_preparations.insert(key.clone()).then_some(key)
    } else {
        None
    };
    let completion = match &request {
        ClientRequest::Switch(SwitchRequest::Commit(id) | SwitchRequest::Cancel(id)) => {
            Some((env.client_id, id.clone()))
        },
        _ => None,
    };
    let response = enqueue(env, env.client_id, request)
        .inspect_err(|_| {
            if let Some(key) = &new_preparation {
                env.session_preparations.remove(key);
            }
        })?
        .recv_timeout(std::time::Duration::from_secs(3))
        .context("client did not answer session request")?;
    // A missing reply leaves ownership uncertain; only confirmed replies release it.
    if response.is_err()
        && let Some(key) = new_preparation
    {
        env.session_preparations.remove(&key);
    }
    if matches!(response, Ok(ClientReply::Complete))
        && let Some(key) = completion
    {
        env.session_preparations.remove(&key);
    }
    response.map_err(anyhow::Error::msg)
}

pub(crate) fn cancel_preparations(env: &mut PluginEnv) {
    for (client, id) in std::mem::take(&mut env.session_preparations) {
        enqueue(
            env,
            client,
            ClientRequest::Switch(SwitchRequest::Cancel(id)),
        )
        .context("cancel disposed plugin's session preparation")
        .non_fatal();
    }
}

fn respond(env: &mut PluginEnv, result: Result<serde_json::Value>) -> Result<()> {
    let response =
        yinz_plugin_api::generated_api::api::plugin_command::SessionDestinationResponse {
            json: serde_json::to_string(&result.map_err(|error| format!("{error:#}")))
                .context("encode session operation response")?,
        };
    write_response(env, &response.encode_to_vec()).context("write session operation response")
}

pub(crate) fn inventory(env: &mut PluginEnv, id: String) -> Result<()> {
    let result = (|| {
        let snapshot = session_list_snapshot(env)?;
        let id = scoped_id(env, &id)?;
        let ClientReply::Inventory(inventory) = request(env, ClientRequest::Inventory(id))? else {
            bail!("Client returned an invalid session inventory");
        };
        let inventory = merge(snapshot, inventory);
        serde_json::to_value(inventory).context("encode session inventory")
    })();
    respond(env, result)
}

fn merge(snapshot: yinz_types::data::SessionListSnapshot, mut inventory: Inventory) -> Inventory {
    // The server has authoritative metadata and saved sessions for its own machine.
    inventory
        .entries
        .retain(|entry| entry.destination != inventory.current_machine);
    for (name, _) in snapshot.resurrectable_sessions {
        if !snapshot.live_sessions.iter().any(|live| live.name == name) {
            inventory.entries.push(Entry {
                destination: Destination::CurrentServer,
                name: Some(name.into_string()),
                status: "Saved".into(),
                current: false,
                saved: true,
            });
        }
    }
    for session in snapshot.live_sessions {
        inventory.entries.push(Entry {
            destination: Destination::CurrentServer,
            name: Some(session.name.into_string()),
            status: if session.is_current_session {
                "Current session"
            } else {
                "Running"
            }
            .into(),
            current: session.is_current_session,
            saved: false,
        });
    }
    inventory
}

pub(crate) fn switch(env: &mut PluginEnv, mut operation: SwitchRequest) -> Result<()> {
    let result = (|| {
        let id = match &mut operation {
            SwitchRequest::Prepare { id, target } => {
                if target.saved {
                    ensure!(
                        target.destination == Destination::CurrentServer,
                        "Only current-server saved sessions can be resurrected"
                    );
                    let snapshot = session_list_snapshot(env)?;
                    ensure!(
                        snapshot
                            .resurrectable_sessions
                            .iter()
                            .any(|(name, _)| name.as_str() == target.name),
                        "Saved session no longer exists"
                    );
                }
                id
            },
            SwitchRequest::Status(id) | SwitchRequest::Commit(id) | SwitchRequest::Cancel(id) => id,
        };
        *id = scoped_id(env, id)?;
        serde_json::to_value(request(env, ClientRequest::Switch(operation))?)
            .context("encode session switch result")
    })();
    respond(env, result)
}

#[cfg(test)]
#[path = "unit/session_destinations.rs"]
mod tests;
