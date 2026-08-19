use revolt_database::{
    util::{permissions::DatabasePermissionQuery, reference::Reference},
    AuditLogEntryAction, Database, Invite, User,
};
use revolt_permissions::{calculate_server_permissions, ChannelPermission};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::Deserialize;
use std::str::FromStr;
use ulid::Ulid;

use crate::util::audit_log_reason::AuditLogReason;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DataCreateVanity {
    /// The slug used in /invite/<code>.
    pub code: String,
}

/// Create a vanity invite for a server.
///
/// The server must have at least five members, and the requested code must be
/// available. Vanity invites use the same invite resolution as regular invites.
#[openapi(tag = "Server Information")]
#[post("/<target>/vanity", data = "<data>")]
pub async fn create(
    db: &State<Database>,
    user: User,
    reason: AuditLogReason,
    target: Reference<'_>,
    data: Json<DataCreateVanity>,
) -> Result<Json<revolt_models::v0::Invite>> {
    if user.bot.is_some() {
        return Err(create_error!(IsBot));
    }

    let server = target.as_server(db).await?;
    let mut query = DatabasePermissionQuery::new(db, &user).server(&server);
    calculate_server_permissions(&mut query)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ManageServer)?;

    if db.fetch_member_count(&server.id).await? < 5 {
        return Err(create_error!(FailedValidation {
            error: "Your server needs at least 5 members to create a vanity URL.".to_string()
        }));
    }

    let code = data.into_inner().code.trim().to_ascii_lowercase();
    if !(3..=32).contains(&code.len())
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_')
        || code.starts_with('-')
        || code.starts_with('_')
        || code.ends_with('-')
        || code.ends_with('_')
    {
        return Err(create_error!(FailedValidation {
            error: "Vanity codes must be 3–32 lowercase letters, numbers, hyphens, or underscores, without a leading or trailing separator.".to_string()
        }));
    }

    // Invite resolution treats ULIDs as discoverable server IDs, so they
    // cannot safely be used as vanity codes.
    if Ulid::from_str(&code).is_ok() {
        return Err(create_error!(FailedValidation {
            error: "That vanity code is reserved.".to_string()
        }));
    }

    if db.fetch_invite(&code).await.is_ok() {
        return Err(create_error!(FailedValidation {
            error: "That vanity code is already taken.".to_string()
        }));
    }

    let channel = server
        .channels
        .first()
        .ok_or(create_error!(NotFound))?
        .clone();
    let invite = Invite::Server {
        code,
        server: server.id.clone(),
        creator: user.id.clone(),
        channel,
    };

    db.insert_invite(&invite).await?;

    AuditLogEntryAction::InviteCreate {
        invite: invite.code().to_string(),
        channel: match &invite {
            Invite::Server { channel, .. } => channel.clone(),
            Invite::Group { channel, .. } => channel.clone(),
        },
    }
    .insert(db, server.id, reason, user.id, None)
    .await;

    Ok(Json(invite.into()))
}
