
use rocket::Route;

mod report_content;

pub fn routes() -> Vec<Route> {
    routes![
        // Reports
        report_content::report_content,
    ]
}
