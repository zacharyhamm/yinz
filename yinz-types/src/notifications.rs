//! Session-local notification arrivals, policy decisions, and final outcomes.
use crate::{
    internal_ids::{NotificationId, PluginId},
    pane_id::PaneId,
    units::UnixMillis,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationSource {
    Toast,
    Osc9,
    Osc99,
    Osc777,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationDelivery {
    Toast,
    Desktop,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationData {
    pub id: NotificationId,
    pub received_at_unix_ms: UnixMillis,
    pub source: NotificationSource,
    pub pane_id: Option<PaneId>,
    pub plugin_id: Option<PluginId>,
    pub title: String,
    pub body: String,
    pub delivery: NotificationDelivery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationDecision {
    Pass,
    Suppress,
    Replace {
        title: Option<String>,
        body: Option<String>,
        delivery: Option<NotificationDelivery>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationResolution {
    pub original: NotificationData,
    pub title: String,
    pub body: String,
    pub delivery: NotificationDelivery,
    pub suppressed: bool,
    pub fallback_reason: Option<String>,
}

pub const MAX_NOTIFICATION_REPLACEMENT_BYTES: usize = 64 * 1024;

impl NotificationResolution {
    pub fn message(&self) -> String {
        match (self.title.is_empty(), self.body.is_empty()) {
            (true, _) => self.body.clone(),
            (_, true) => self.title.clone(),
            _ => format!("{}: {}", self.title, self.body),
        }
    }
    pub fn from_decision(
        original: NotificationData,
        decision: NotificationDecision,
        fallback_reason: Option<String>,
    ) -> Self {
        let mut result = Self {
            title: original.title.clone(),
            body: original.body.clone(),
            delivery: original.delivery,
            original,
            suppressed: false,
            fallback_reason,
        };
        match decision {
            NotificationDecision::Pass => {},
            NotificationDecision::Suppress => result.suppressed = true,
            NotificationDecision::Replace {
                title,
                body,
                delivery,
            } => {
                if let Some(title) = title {
                    result.title = title;
                }
                if let Some(body) = body {
                    result.body = body;
                }
                if let Some(delivery) = delivery {
                    result.delivery = delivery;
                }
            },
        }
        result
    }
}
