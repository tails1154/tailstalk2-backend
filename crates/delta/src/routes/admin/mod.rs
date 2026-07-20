use revolt_rocket_okapi::revolt_okapi::openapi3::OpenApi;
use rocket::Route;

mod auth;
mod reports;
mod stats;
mod users;

pub fn routes() -> (Vec<Route>, OpenApi) {
    openapi_get_routes_spec![
        stats::admin_stats,
        reports::admin_reports,
        reports::admin_resolve_report,
        reports::admin_dismiss_report,
        users::admin_search_users,
        users::admin_ban_user,
        users::admin_unban_user,
        users::admin_suspend_user,
        users::admin_warn_user,
        users::admin_clear_warnings,
        users::admin_delete_warning,
    ]
}
