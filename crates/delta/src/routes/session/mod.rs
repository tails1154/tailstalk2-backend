use revolt_rocket_okapi::revolt_okapi::openapi3::OpenApi;
use rocket::Route;

pub mod edit;
pub mod fetch_all;
pub mod login;
pub mod logout;
pub mod passkeys;
pub mod revoke;
pub mod revoke_all;

pub fn routes() -> (Vec<Route>, OpenApi) {
    openapi_get_routes_spec![
        login::login,
        passkeys::register_start,
        passkeys::register_finish,
        passkeys::list,
        passkeys::delete,
        passkeys::login_start,
        passkeys::login_finish,
        logout::logout,
        fetch_all::fetch_all,
        revoke::revoke,
        revoke_all::revoke_all,
        edit::edit
    ]
}
