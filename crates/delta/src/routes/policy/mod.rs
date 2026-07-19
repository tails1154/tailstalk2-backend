
use rocket::Route;

mod acknowledge_policy_changes;

pub fn routes() -> Vec<Route> {
    routes![
        // Policy
        acknowledge_policy_changes::acknowledge_policy_changes,
    ]
}
