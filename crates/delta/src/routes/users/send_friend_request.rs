// use revolt_database::util::reference::Reference;
use revolt_database::{Database, User, AMQP};
use revolt_models::v0;
use revolt_result::{create_error, Result};
use rocket::serde::json::Json;
use rocket::State;

/// # Send Friend Request
///
/// Send a friend request to another user.
#[openapi(tag = "Relationships")]
#[post("/friend", data = "<data>")]
pub async fn send_friend_request(
    db: &State<Database>,
    amqp: &State<AMQP>,
    mut user: User,
    data: Json<v0::DataSendFriendRequest>,
) -> Result<Json<v0::User>> {
    let username = data.username.split_once('#').map_or(data.username.as_str(), |(username, _)| username);
    if !username.is_empty() {
        let mut target = db.fetch_user_by_username(username, "0000").await?;

        if user.bot.is_some() || target.bot.is_some() {
            return Err(create_error!(IsBot));
        }

        user.add_friend(db, amqp, &mut target).await?;
        Ok(Json(target.into(db, &user).await))
    } else { Err(create_error!(InvalidProperty)) }
}
