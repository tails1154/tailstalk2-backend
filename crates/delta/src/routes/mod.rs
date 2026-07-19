use revolt_config::Settings;
pub use rocket::http::Status;
pub use rocket::response::Redirect;
use rocket::{Build, Rocket};

mod bots;
mod channels;
mod customisation;
mod invites;
mod onboard;
mod policy;
mod push;
mod root;
mod safety;
mod servers;
mod sync;
mod users;
mod webhooks;
mod account;
mod session;
mod mfa;

pub fn mount(config: Settings, mut rocket: Rocket<Build>) -> Rocket<Build> {
    rocket = rocket
        .mount("", routes![root::root])
        .mount("/users", users::routes())
        .mount("/bots", bots::routes())
        .mount("/channels", channels::routes())
        .mount("/servers", servers::routes())
        .mount("/invites", invites::routes())
        .mount("/custom", customisation::routes())
        .mount("/safety", safety::routes())
        .mount("/auth/account", account::routes())
        .mount("/auth/session", session::routes())
        .mount("/auth/mfa", mfa::routes())
        .mount("/onboard", onboard::routes())
        .mount("/policy", policy::routes())
        .mount("/push", push::routes())
        .mount("/sync", sync::routes());

    if config.features.webhooks_enabled {
        rocket = rocket.mount("/webhooks", webhooks::routes());
    }

    rocket
}
