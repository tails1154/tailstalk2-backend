
use rocket::Route;

mod emoji_create;
mod emoji_delete;
mod emoji_edit;
mod emoji_fetch;

pub fn routes() -> Vec<Route> {
    routes![
        emoji_create::create_emoji,
        emoji_delete::delete_emoji,
        emoji_edit::edit_emoji,
        emoji_fetch::fetch_emoji
    ]
}
