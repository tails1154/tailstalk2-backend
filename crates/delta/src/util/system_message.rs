use revolt_database::{Channel, Database, Message};

const SYSTEM_USER_ID: &str = "01M6HG5PZABFWZHAKVZ1KM896S";

/// Send a DM from the configured TailsTalk 2 System account.
///
/// Notifications are best-effort so an unavailable System account never makes
/// an unrelated admin operation fail.
pub async fn send_system_dm(db: &Database, recipient_id: &str, content: impl Into<String>) {
    let (Ok(recipient), Ok(system_user)) = (
        db.fetch_user(recipient_id).await,
        db.fetch_user(SYSTEM_USER_ID).await,
    ) else {
        return;
    };
    let Ok(channel) = Channel::create_dm(db, &system_user, &recipient).await else {
        return;
    };

    let author = system_user.clone().into(db, Some(&system_user)).await;
    let model_user = system_user.clone().into_known_static(false).await;
    let mut message = Message {
        id: ulid::Ulid::new().to_string(),
        channel: channel.id().to_string(),
        author: system_user.id.clone(),
        content: Some(content.into()),
        ..Default::default()
    };
    let _ = message
        .send(
            db,
            None,
            revolt_models::v0::MessageAuthor::User(&author),
            Some(model_user),
            None,
            &channel,
            false,
        )
        .await;
}
