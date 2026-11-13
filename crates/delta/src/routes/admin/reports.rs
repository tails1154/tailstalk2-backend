use revolt_database::{
    mongodb::bson::{self, doc},
    Channel, Database, Message,
};
use revolt_result::Result;
use rocket::serde::json::Json;
use rocket::State;
use serde::{Deserialize, Serialize};

use super::auth::ReportAccess;

#[derive(Serialize, JsonSchema, Debug)]
pub struct AdminReport {
    pub id: String,
    pub author_id: String,
    pub author_name: String,
    pub content_type: String,
    pub content_id: String,
    pub content_name: String,
    pub message_content: Option<String>,
    pub report_reason: String,
    pub additional_context: String,
    pub status: String,
    pub notes: String,
    pub server_id: Option<String>,
    pub server_name: Option<String>,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub struct ResolveReportData {
    pub notes: Option<String>,
    pub action: Option<String>,
    pub action_reason: Option<String>,
    pub suspend_hours: Option<i64>,
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
        author_name: String::new(),
        content_type,
        content_id,
        content_name: String::new(),
        message_content: None,
        report_reason,
        additional_context: doc.get_str("additional_context").unwrap_or("").to_string(),
        status: doc.get_str("status").unwrap_or("Created").to_string(),
        notes: doc.get_str("notes").unwrap_or("").to_string(),
        server_id: None,
        server_name: None,
    })
}

/// # List Reports
///
/// Fetch all safety reports. Requires HTTP Basic Auth.
#[openapi(tag = "Admin")]
#[get("/reports")]
pub async fn admin_reports(
    _access: ReportAccess,
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
                if let Some(mut report) = report_from_doc(&doc) {
                    let users = mongo.col::<bson::Document>("users");
                    if let Ok(user) = users.find_one(doc! { "_id": &report.author_id }).await {
                        if let Some(user) = user {
                            report.author_name = user
                                .get_str("display_name")
                                .ok()
                                .filter(|name| !name.is_empty())
                                .or_else(|| user.get_str("username").ok())
                                .unwrap_or("Unknown user")
                                .to_string();
                        }
                    }

                    let snapshots = mongo.col::<bson::Document>("safety_snapshots");
                    if let Ok(snapshot) = snapshots.find_one(doc! { "report_id": &report.id }).await
                    {
                        if let Some(snapshot) = snapshot {
                            if let Ok(content) = snapshot.get_document("content") {
                                report.message_content =
                                    content.get_str("content").ok().map(ToString::to_string);
                                report.content_name = report
                                    .message_content
                                    .clone()
                                    .unwrap_or_else(|| report.content_id.clone());

                                // Message snapshots retain their channel, which is enough to
                                // associate a report with a server for admin grouping.
                                if let Ok(channel_id) = content.get_str("channel") {
                                    if let Ok(Some(channel)) = mongo
                                        .col::<bson::Document>("channels")
                                        .find_one(doc! { "_id": channel_id })
                                        .await
                                    {
                                        report.server_id =
                                            channel.get_str("server").ok().map(ToString::to_string);
                                    }
                                }
                                if report.server_id.is_none()
                                    && content.get_str("_type").ok() == Some("Server")
                                {
                                    report.server_id =
                                        content.get_str("_id").ok().map(ToString::to_string);
                                }
                            }
                        }
                    }

                    if let Some(server_id) = &report.server_id {
                        if let Ok(Some(server)) = mongo
                            .col::<bson::Document>("servers")
                            .find_one(doc! { "_id": server_id })
                            .await
                        {
                            report.server_name =
                                server.get_str("name").ok().map(ToString::to_string);
                        }
                    }

                    if report.content_name.is_empty() {
                        report.content_name = report.content_id.clone();
                    }
                    if report.author_name.is_empty() {
                        report.author_name = report.author_id.clone();
                    }
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
    _access: ReportAccess,
    db: &State<Database>,
    id: String,
    data: Json<ResolveReportData>,
) -> Result<Json<serde_json::Value>> {
    match db.inner() {
        Database::MongoDb(mongo) => {
            let report = mongo
                .col::<bson::Document>("safety_reports")
                .find_one(doc! { "_id": &id })
                .await
                .map_err(|_| revolt_result::create_error!(InternalError))?
                .ok_or(revolt_result::create_error!(NotFound))?;

            let reported_user_id = match report
                .get_document("content")
                .ok()
                .and_then(|content| content.get_str("type").ok())
            {
                Some("User") => report
                    .get_document("content")
                    .ok()
                    .and_then(|content| content.get_str("id").ok())
                    .map(str::to_owned),
                Some("Message") => mongo
                    .col::<bson::Document>("safety_snapshots")
                    .find_one(doc! { "report_id": &id, "content.author": { "$exists": true } })
                    .await
                    .ok()
                    .flatten()
                    .and_then(|snapshot| snapshot.get_document("content").ok().cloned())
                    .and_then(|content| content.get_str("author").ok().map(str::to_owned)),
                Some("Server") => {
                    let server_id = report
                        .get_document("content")
                        .ok()
                        .and_then(|content| content.get_str("id").ok());
                    match server_id {
                        Some(server_id) => mongo
                            .col::<bson::Document>("servers")
                            .find_one(doc! { "_id": server_id })
                            .await
                            .ok()
                            .flatten()
                            .and_then(|server| server.get_str("owner").ok().map(str::to_owned)),
                        None => None,
                    }
                }
                _ => None,
            };

            if let (Some(action), Some(target_id)) =
                (data.action.as_deref(), reported_user_id.as_deref())
            {
                let reason = data
                    .action_reason
                    .as_deref()
                    .unwrap_or("Action taken after review of a report.");
                match action {
                    "warn" => {
                        mongo
                            .col::<bson::Document>("users")
                            .update_one(
                                doc! { "_id": &target_id },
                                doc! { "$push": { "admin_warnings": {
                                    "reason": reason,
                                    "created_at": chrono::Utc::now().to_rfc3339(),
                                } } },
                            )
                            .await
                            .map_err(|_| revolt_result::create_error!(InternalError))?;
                    }
                    "suspend" => {
                        let hours = data.suspend_hours.unwrap_or(24).max(1);
                        let until = iso8601_timestamp::Timestamp::now_utc()
                            + iso8601_timestamp::Duration::hours(hours);
                        let mut target = db.fetch_user(&target_id).await?;
                        target
                            .suspend(db, Some(until), Some(vec![reason.to_string()]))
                            .await?;
                        mongo
                            .col::<bson::Document>("users")
                            .update_one(
                                doc! { "_id": &target_id },
                                doc! { "$set": { "admin_suspend_reason": reason } },
                            )
                            .await
                            .map_err(|_| revolt_result::create_error!(InternalError))?;
                    }
                    "ban" => {
                        let mut set = doc! { "flags": 4i32 };
                        set.insert("admin_ban_reason", reason);
                        mongo
                            .col::<bson::Document>("users")
                            .update_one(doc! { "_id": &target_id }, doc! { "$set": set })
                            .await
                            .map_err(|_| revolt_result::create_error!(InternalError))?;
                    }
                    "none" => {}
                    _ => return Err(revolt_result::create_error!(InvalidProperty)),
                }
            }

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

            // Notify the reporter from the dedicated System account. This is intentionally
            // best-effort: report resolution must still succeed if the account is unavailable.
            let reported_user_name = match reported_user_id.as_deref() {
                Some(target_id) => db
                    .fetch_user(target_id)
                    .await
                    .map(|user| user.username)
                    .unwrap_or_else(|_| "the reported account".to_owned()),
                None => "the reported account".to_owned(),
            };
            if let (Ok(reporter), Ok(system_user)) = (
                db.fetch_user(report.get_str("author_id").unwrap_or_default()).await,
                db.fetch_user("01M6HG5PZABFWZHAKVZ1KM896S").await,
            ) {
                if let Ok(channel) = Channel::create_dm(db, &system_user, &reporter).await {
                    let system_author = system_user.clone().into(db, Some(&system_user)).await;
                    let system_model = system_user.clone().into_known_static(false).await;
                    let mut message = Message {
                        id: ulid::Ulid::new().to_string(),
                        channel: channel.id().to_string(),
                        author: system_user.id.clone(),
                        content: Some(format!(
                            "Your report to {} was acted upon. Thanks for keeping TailsTalk 2 safe!",
                            reported_user_name
                        )),
                        ..Default::default()
                    };
                    let _ = message
                        .send(
                            db,
                            None,
                            revolt_models::v0::MessageAuthor::User(&system_author),
                            Some(system_model),
                            None,
                            &channel,
                            false,
                        )
                        .await;
                }
            }
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
    _access: ReportAccess,
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
