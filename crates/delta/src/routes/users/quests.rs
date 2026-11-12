use std::time::{SystemTime, UNIX_EPOCH};

use revolt_database::{
    mongodb::bson::{doc, Document},
    Database, User,
};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

/// Per-window message counters, one document per user, quest and window.
const PROGRESS: &str = "tails_quest_progress";
/// Shared per-user economy document. `xp` drives levels; `tails_points` is spent in the shop.
const BALANCES: &str = "user_xp";

/// A repeating quest. Progress resets at each window boundary (fixed windows, UTC-aligned).
pub struct QuestDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub target: i64,
    pub window_seconds: i64,
    pub reward_points: i64,
}

pub const QUESTS: &[QuestDefinition] = &[
    QuestDefinition {
        id: "messages_hour_10",
        name: "Warm Up",
        description: "Send 10 messages within one hour",
        target: 10,
        window_seconds: 3600,
        reward_points: 20,
    },
    QuestDefinition {
        id: "messages_hour_50",
        name: "Chatterbox",
        description: "Send 50 messages within one hour",
        target: 50,
        window_seconds: 3600,
        reward_points: 100,
    },
    QuestDefinition {
        id: "messages_day_300",
        name: "Daily Regular",
        description: "Send 300 messages within one day",
        target: 300,
        window_seconds: 86_400,
        reward_points: 500,
    },
];

/// Start of the fixed window that contains `now_secs`.
pub(crate) fn window_start(now_secs: i64, window_seconds: i64) -> i64 {
    now_secs - now_secs.rem_euclid(window_seconds)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn progress_id(user_id: &str, quest: &QuestDefinition, window: i64) -> String {
    format!("{user_id}:{}:{window}", quest.id)
}

fn mongo(db: &Database) -> Result<&revolt_database::MongoDb> {
    match db {
        Database::MongoDb(db) => Ok(db),
        _ => Err(create_error!(InternalError)),
    }
}

/// Count one sent message against every quest. Called after a successful message send.
pub(crate) async fn record_message(db: &Database, user_id: &str) {
    let Database::MongoDb(mongo) = db else {
        return;
    };
    let now = now_secs();
    for quest in QUESTS {
        let window = window_start(now, quest.window_seconds);
        let _ = mongo
            .col::<Document>(PROGRESS)
            .update_one(
                doc! { "_id": progress_id(user_id, quest, window) },
                doc! {
                    "$inc": { "count": 1_i64 },
                    "$setOnInsert": {
                        "user_id": user_id,
                        "quest_id": quest.id,
                        "window_start": window,
                    },
                },
            )
            .upsert(true)
            .await;
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct QuestResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub progress: i64,
    pub target: i64,
    pub reward_points: i64,
    pub window_seconds: i64,
    /// Unix seconds when the current window ends and progress resets.
    pub resets_at: i64,
    pub completed: bool,
    pub claimed: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct QuestsResponse {
    pub tails_points: i64,
    pub quests: Vec<QuestResponse>,
}

async fn tails_points(db: &Database, user_id: &str) -> Result<i64> {
    let balance = mongo(db)?
        .col::<Document>(BALANCES)
        .find_one(doc! { "_id": user_id })
        .await
        .map_err(|_| create_error!(InternalError))?;
    Ok(balance
        .and_then(|document| document.get_i64("tails_points").ok())
        .unwrap_or(0))
}

/// List the current window's quests with progress, plus the user's Tails Points balance.
#[openapi(tag = "Quests")]
#[get("/quests")]
pub async fn list(db: &State<Database>, user: User) -> Result<Json<QuestsResponse>> {
    let now = now_secs();
    let mut quests = Vec::with_capacity(QUESTS.len());
    for quest in QUESTS {
        let window = window_start(now, quest.window_seconds);
        let progress = mongo(db)?
            .col::<Document>(PROGRESS)
            .find_one(doc! { "_id": progress_id(&user.id, quest, window) })
            .await
            .map_err(|_| create_error!(InternalError))?;
        let count = progress
            .as_ref()
            .and_then(|document| document.get_i64("count").ok())
            .unwrap_or(0)
            .min(quest.target);
        let claimed = progress
            .as_ref()
            .and_then(|document| document.get_bool("claimed").ok())
            .unwrap_or(false);
        quests.push(QuestResponse {
            id: quest.id.to_owned(),
            name: quest.name.to_owned(),
            description: quest.description.to_owned(),
            progress: count,
            target: quest.target,
            reward_points: quest.reward_points,
            window_seconds: quest.window_seconds,
            resets_at: window + quest.window_seconds,
            completed: count >= quest.target,
            claimed,
        });
    }
    Ok(Json(QuestsResponse {
        tails_points: tails_points(db, &user.id).await?,
        quests,
    }))
}

/// Claim the reward for a completed quest in the current window. Each window pays out once.
#[openapi(tag = "Quests")]
#[post("/quests/<quest_id>/claim")]
pub async fn claim(
    db: &State<Database>,
    user: User,
    quest_id: String,
) -> Result<Json<serde_json::Value>> {
    if user.bot.is_some() {
        return Err(create_error!(IsBot));
    }
    let quest = QUESTS
        .iter()
        .find(|quest| quest.id == quest_id)
        .ok_or(create_error!(NotFound))?;
    let window = window_start(now_secs(), quest.window_seconds);
    let database = mongo(db)?;

    // Mark claimed only if the target is met and not yet claimed. This guards against double claims.
    let marked = database
        .col::<Document>(PROGRESS)
        .update_one(
            doc! {
                "_id": progress_id(&user.id, quest, window),
                "count": { "$gte": quest.target },
                "claimed": { "$ne": true },
            },
            doc! { "$set": { "claimed": true, "claimed_at": now_secs() } },
        )
        .await
        .map_err(|_| create_error!(InternalError))?;
    if marked.modified_count != 1 {
        return Err(create_error!(FailedValidation {
            error: "Quest is not complete or was already claimed".to_owned()
        }));
    }

    database
        .col::<Document>(BALANCES)
        .update_one(
            doc! { "_id": &user.id },
            doc! { "$inc": { "tails_points": quest.reward_points } },
        )
        .upsert(true)
        .await
        .map_err(|_| create_error!(InternalError))?;

    Ok(Json(json!({
        "ok": true,
        "points_awarded": quest.reward_points,
        "tails_points": tails_points(db, &user.id).await?,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_start_aligns_to_fixed_boundaries() {
        assert_eq!(window_start(3_600 * 5 + 1_234, 3_600), 3_600 * 5);
        assert_eq!(window_start(3_600 * 5, 3_600), 3_600 * 5);
        assert_eq!(window_start(86_400 + 59, 86_400), 86_400);
    }

    #[test]
    fn every_quest_has_a_positive_target_and_reward() {
        for quest in QUESTS {
            assert!(quest.target > 0, "{} needs a target", quest.id);
            assert!(quest.reward_points > 0, "{} needs a reward", quest.id);
            assert!(quest.window_seconds > 0, "{} needs a window", quest.id);
        }
    }

    #[test]
    fn quest_ids_are_unique() {
        for (index, quest) in QUESTS.iter().enumerate() {
            assert!(
                QUESTS[index + 1..].iter().all(|other| other.id != quest.id),
                "duplicate quest id {}",
                quest.id
            );
        }
    }
}
