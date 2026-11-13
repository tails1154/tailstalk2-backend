use revolt_rocket_okapi::revolt_okapi::openapi3::OpenApi;
use rocket::Route;

pub(crate) mod auth;
mod reports;
mod safety;
mod servers;
mod stats;
mod profile_decorations;
mod users;
mod whatsnew;

pub fn routes() -> (Vec<Route>, OpenApi) {
    let (mut routes, spec) = openapi_get_routes_spec![
        stats::admin_stats,
        profile_decorations::list,
        profile_decorations::create,
        reports::admin_reports,
        reports::admin_resolve_report,
        reports::admin_dismiss_report,
        safety::list_flags,
        safety::review_flag,
        safety::panel_access,
        safety::school_account_info,
        safety::suspend_school_account,
        safety::unsuspend_school_account,
        safety::ban_school_account,
        safety::flag_context,
        servers::admin_servers,
        servers::admin_suspend_server,
        servers::admin_delete_server,
        servers::admin_create_server_invite,
        users::admin_search_users,
        users::admin_generate_password_reset,
        users::admin_set_badge,
        users::admin_set_access,
        users::admin_ban_user,
        users::admin_unban_user,
        users::admin_suspend_user,
        users::admin_unsuspend_user,
        users::admin_verify_email,
        users::admin_pending_verification,
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
    ];
    routes.extend(safety::oauth_routes());
    (routes, spec)
}
