
use rocket::Route;

mod get_settings;
mod get_unreads;
mod set_settings;

pub fn routes() -> Vec<Route> {
    routes![get_settings::fetch, set_settings::set, get_unreads::unreads]
}
