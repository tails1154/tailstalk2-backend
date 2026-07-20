use revolt_database::{
    mongodb::bson::{self, doc},
    Database,
};
use revolt_result::Result;
use rocket::State;
use rocket::serde::json::Json;
use serde::{Deserialize, Serialize};

use super::auth::AdminUser;

#[derive(Serialize, JsonSchema, Debug)]
pub struct AdminReport {
    pub id: String,
    pub author_id: String,
    pub content_type: String,
    pub content_id: String,
    pub report_reason: String,
    pub additional_context: String,
    pub status: String,
    pub notes: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub struct ResolveReportData {
    pub notes: Option<String>,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub struct DismissReportData {
    pub rejection_reason: String,
}

fn report_from_doc(doc: &bson::Document) -> Option<AdminReport> {
    let content = doc.get_document("content").ok()?;
    let content_type = content.get_str("type").unwrap_or("Unknown").to_string();
    let content_id = content.get_str("id").unwrap_or("").to_string();
    let report_reason = content
        .get_str("report_reason")
        .unwrap_or("NoneSpecified")
        .to_string();

    Some(AdminReport {
        id: doc.get_str("_id").ok()?.to_string(),
        author_id: doc.get_str("author_id").ok()?.to_string(),
        content_type,
        content_id,
        report_reason,
        additional_context: doc
            .get_str("additional_context")
            .unwrap_or("")
            .to_string(),
        status: doc.get_str("status").unwrap_or("Created").to_string(),
        notes: doc.get_str("notes").unwrap_or("").to_string(),
    })
}

/// # List Reports
///
/// Fetch all safety reports. Requires HTTP Basic Auth.
#[openapi(tag = "Admin")]
#[get("/reports")]
pub async fn admin_reports(
    _admin: AdminUser,
    db: &State<Database>,
) -> Result<Json<Vec<AdminReport>>> {
    let reports = match db.inner() {
        Database::MongoDb(mongo) => {
            let mut cursor = mongo
                .col::<bson::Document>("safety_reports")
                .find(doc! {})
                .sort(doc! { "_id": -1 })
                .limit(50)
                .await
                .map_err(|_| revolt_result::create_error!(InternalError))?;

            let mut result = Vec::new();
            use futures::StreamExt;
            while let Some(Ok(doc)) = cursor.next().await {
                if let Some(report) = report_from_doc(&doc) {
                    result.push(report);
                }
            }
            result
        }
        _ => Vec::new(),
    };

    Ok(Json(reports))
}

/// # Resolve Report
///
/// Mark a report as resolved.
#[openapi(tag = "Admin")]
#[post("/reports/<id>/resolve", data = "<data>")]
pub async fn admin_resolve_report(
    _admin: AdminUser,
    db: &State<Database>,
    id: String,
    data: Json<ResolveReportData>,
) -> Result<Json<serde_json::Value>> {
    match db.inner() {
        Database::MongoDb(mongo) => {
            let mut update = doc! {
                "status": "Resolved",
                "closed_at": bson::to_bson(
                    &iso8601_timestamp::Timestamp::now_utc()
                ).unwrap_or_default(),
            };
            if let Some(ref notes) = data.notes {
                update.insert("notes", notes.as_str());
            }
            mongo
                .col::<bson::Document>("safety_reports")
                .update_one(doc! { "_id": &id }, doc! { "$set": update })
                .await
                .map_err(|_| revolt_result::create_error!(InternalError))?;
        }
        _ => {}
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// # Dismiss Report
///
/// Dismiss a report with a reason.
#[openapi(tag = "Admin")]
#[post("/reports/<id>/dismiss", data = "<data>")]
pub async fn admin_dismiss_report(
    _admin: AdminUser,
    db: &State<Database>,
    id: String,
    data: Json<DismissReportData>,
) -> Result<Json<serde_json::Value>> {
    match db.inner() {
        Database::MongoDb(mongo) => {
            let reason: &str = &data.rejection_reason;
            mongo
                .col::<bson::Document>("safety_reports")
                .update_one(
                    doc! { "_id": &id },
                    doc! {
                        "$set": {
                            "status": "Rejected",
                            "rejection_reason": reason,
                            "closed_at": bson::to_bson(
                                &iso8601_timestamp::Timestamp::now_utc()
                            ).unwrap_or_default(),
                        }
                    },
                )
                .await
                .map_err(|_| revolt_result::create_error!(InternalError))?;
        }
        _ => {}
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}
