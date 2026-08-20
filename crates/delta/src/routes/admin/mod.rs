use revolt_rocket_okapi::revolt_okapi::openapi3::OpenApi;
use rocket::Route;

pub(crate) mod auth;
mod reports;
mod stats;
mod users;
mod whatsnew;

pub fn routes() -> (Vec<Route>, OpenApi) {
    openapi_get_routes_spec![
        stats::admin_stats,
        reports::admin_reports,
        reports::admin_resolve_report,
        reports::admin_dismiss_report,
        users::admin_search_users,
        users::admin_set_badge,
        users::admin_ban_user,
        users::admin_unban_user,
        users::admin_suspend_user,
        users::admin_warn_user,
        users::admin_clear_warnings,
        users::admin_delete_warning,
        whatsnew::admin_get_whatsnew,
        whatsnew::admin_set_whatsnew,
        crate::routes::discovery::admin_list,
        crate::routes::discovery::approve,
        crate::routes::discovery::deny,
        crate::routes::feature_requests::admin_get,
        crate::routes::feature_requests::approve,
        crate::routes::feature_requests::deny,
        crate::routes::decorations::admin_create,
        crate::routes::decorations::admin_delete,
    ]
}
