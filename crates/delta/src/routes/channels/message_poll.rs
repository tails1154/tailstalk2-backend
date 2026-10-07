use revolt_database::util::{permissions::DatabasePermissionQuery, reference::Reference};
use revolt_database::{Database, PartialMessage, User};
use revolt_models::v0;
use revolt_permissions::{calculate_channel_permissions, ChannelPermission};
use revolt_result::{create_error, Result};
use rocket::serde::json::Json;
use rocket::State;
use std::time::{SystemTime, UNIX_EPOCH};

/// Vote in a native message poll.
#[openapi(tag = "Messaging")]
#[post("/<target>/messages/<msg>/poll/vote", data = "<vote>")]
pub async fn vote(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    msg: Reference<'_>,
    vote: Json<v0::PollVote>,
) -> Result<Json<v0::Message>> {
    let channel = target.as_channel(db).await?;
    let mut permissions = DatabasePermissionQuery::new(db, &user).channel(&channel);
    calculate_channel_permissions(&mut permissions)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ViewChannel)?;

    let mut message = msg.as_message_in_channel(db, channel.id()).await?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let poll = message
        .poll
        .as_mut()
        .ok_or(create_error!(InvalidOperation))?;
    if poll.closed || poll.closes_at.is_some_and(|closes_at| now >= closes_at) {
        return Err(create_error!(InvalidOperation));
    }

    let option_exists = poll
        .options
        .iter()
        .any(|option| option.id == vote.option_id);
    if !option_exists {
        return Err(create_error!(InvalidProperty));
    }

    if !poll.multiple && vote.selected {
        for voters in poll.votes.values_mut() {
            voters.shift_remove(&user.id);
        }
    }

    let voters = poll.votes.entry(vote.option_id.clone()).or_default();
    if vote.selected {
        voters.insert(user.id.clone());
    } else {
        voters.shift_remove(&user.id);
    }

    message
        .update(
            db,
            PartialMessage {
                poll: message.poll.clone(),
                ..Default::default()
            },
            vec![],
        )
        .await?;

    Ok(Json(message.into_model(None, None)))
}

/// Close a native message poll.
#[openapi(tag = "Messaging")]
#[post("/<target>/messages/<msg>/poll/close")]
pub async fn close(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    msg: Reference<'_>,
) -> Result<Json<v0::Message>> {
    let channel = target.as_channel(db).await?;
    let mut message = msg.as_message_in_channel(db, channel.id()).await?;

    if message.author != user.id {
        return Err(create_error!(NotFound));
    }

    let poll = message
        .poll
        .as_mut()
        .ok_or(create_error!(InvalidOperation))?;
    poll.closed = true;
    message
        .update(
            db,
            PartialMessage {
                poll: message.poll.clone(),
                ..Default::default()
            },
            vec![],
        )
        .await?;

    Ok(Json(message.into_model(None, None)))
}
