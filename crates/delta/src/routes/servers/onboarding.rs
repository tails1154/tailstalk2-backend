use revolt_database::{
    mongodb::bson::{doc, Document},
    util::{permissions::DatabasePermissionQuery, reference::Reference},
    Database, PartialMember, User,
};
use revolt_permissions::{calculate_server_permissions, ChannelPermission};
use revolt_result::{create_database_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ServerOnboarding {
    pub enabled: bool,
    #[serde(default)]
    pub completed: bool,
    pub title: String,
    pub message: String,
    pub rules: String,
    #[serde(default)]
    pub questions: Vec<OnboardingQuestion>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct OnboardingQuestion {
    pub id: String,
    pub prompt: String,
    #[serde(default)]
    pub multiple: bool,
    pub options: Vec<OnboardingOption>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct OnboardingOption {
    pub id: String,
    pub label: String,
    pub role_id: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct CompleteOnboarding {
    #[serde(default)]
    pub answers: HashMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct CompleteOnboardingResponse {
    pub roles: Vec<String>,
}

fn validate_questions(
    questions: &[OnboardingQuestion],
    server: &revolt_database::Server,
) -> Result<()> {
    let role_ids = server.roles.keys().cloned().collect::<HashSet<_>>();
    validate_question_config(questions, &role_ids, &server.id)
}

fn validate_question_config(
    questions: &[OnboardingQuestion],
    role_ids: &HashSet<String>,
    default_role_id: &str,
) -> Result<()> {
    let mut question_ids = HashSet::new();
    for question in questions {
        if question.id.trim().is_empty() || !question_ids.insert(&question.id) {
            return Err(revolt_result::create_error!(InvalidProperty));
        }

        let mut option_ids = HashSet::new();
        for option in &question.options {
            if option.id.trim().is_empty()
                || option.label.trim().is_empty()
                || !option_ids.insert(&option.id)
                || !role_ids.contains(&option.role_id)
                || option.role_id == default_role_id
            {
                return Err(revolt_result::create_error!(InvalidProperty));
            }
        }
    }
    Ok(())
}

fn selected_roles(
    questions: &[OnboardingQuestion],
    answers: HashMap<String, Vec<String>>,
) -> Result<Vec<String>> {
    let mut selected_roles = HashSet::new();
    for (question_id, option_ids) in answers {
        let question = questions
            .iter()
            .find(|question| question.id == question_id)
            .ok_or_else(|| revolt_result::create_error!(InvalidProperty))?;
        if !question.multiple && option_ids.len() > 1 {
            return Err(revolt_result::create_error!(InvalidProperty));
        }
        for option_id in option_ids {
            let option = question
                .options
                .iter()
                .find(|option| option.id == option_id)
                .ok_or_else(|| revolt_result::create_error!(InvalidProperty))?;
            selected_roles.insert(option.role_id.clone());
        }
    }

    let mut selected_roles = selected_roles.into_iter().collect::<Vec<_>>();
    selected_roles.sort();
    Ok(selected_roles)
}

/// Fetch onboarding settings for a server member.
#[openapi(tag = "Server Information")]
#[get("/<target>/onboarding")]
pub async fn fetch(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
) -> Result<Json<ServerOnboarding>> {
    let server = target.as_server(db).await?;
    db.fetch_member(&server.id, &user.id).await?;

    let mut settings = match &**db {
        Database::MongoDb(mongo) => mongo
            .col::<ServerOnboarding>("server_onboarding")
            .find_one(doc! { "_id": &server.id })
            .await
            .map_err(|_| create_database_error!("find_one", "server_onboarding"))?
            .unwrap_or_else(|| ServerOnboarding {
                enabled: false,
                title: format!("Welcome to {}", server.name),
                ..Default::default()
            }),
        _ => ServerOnboarding {
            enabled: false,
            title: format!("Welcome to {}", server.name),
            ..Default::default()
        },
    };

    if let Database::MongoDb(mongo) = &**db {
        settings.completed = mongo
            .col::<Document>("server_onboarding_completions")
            .find_one(doc! {
                "_id.server": &server.id,
                "_id.user": &user.id,
            })
            .await
            .map_err(|_| create_database_error!("find_one", "server_onboarding_completions"))?
            .is_some();
    }

    Ok(Json(settings))
}

/// Update onboarding settings. Requires ManageServer.
#[openapi(tag = "Server Information")]
#[patch("/<target>/onboarding", data = "<data>")]
pub async fn update(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    data: Json<ServerOnboarding>,
) -> Result<Json<ServerOnboarding>> {
    let server = target.as_server(db).await?;
    let mut query = DatabasePermissionQuery::new(db, &user).server(&server);
    calculate_server_permissions(&mut query)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ManageServer)?;

    let settings = data.into_inner();
    validate_questions(&settings.questions, &server)?;
    match &**db {
        Database::MongoDb(mongo) => {
            let mut document = revolt_database::mongodb::bson::to_document(&settings)
                .map_err(|_| create_database_error!("serialize", "server_onboarding"))?;
            document.insert("_id", &server.id);
            mongo
                .col::<Document>("server_onboarding")
                .replace_one(doc! { "_id": &server.id }, document)
                .upsert(true)
                .await
                .map_err(|_| create_database_error!("replace_one", "server_onboarding"))?;
        }
        _ => return Err(create_database_error!("replace_one", "server_onboarding")),
    }

    Ok(Json(settings))
}

/// Apply the roles selected in onboarding.
#[openapi(tag = "Server Information")]
#[post("/<target>/onboarding/complete", data = "<data>")]
pub async fn complete(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    data: Json<CompleteOnboarding>,
) -> Result<Json<CompleteOnboardingResponse>> {
    let server = target.as_server(db).await?;
    let mut member = db.fetch_member(&server.id, &user.id).await?;
    let settings = match &**db {
        Database::MongoDb(mongo) => mongo
            .col::<ServerOnboarding>("server_onboarding")
            .find_one(doc! { "_id": &server.id })
            .await
            .map_err(|_| create_database_error!("find_one", "server_onboarding"))?
            .unwrap_or_default(),
        _ => ServerOnboarding::default(),
    };

    validate_questions(&settings.questions, &server)?;

    let selected_roles = selected_roles(&settings.questions, data.into_inner().answers)?;

    let mut roles = member.roles.clone();
    roles.extend(selected_roles.iter().cloned());
    roles.sort();
    roles.dedup();
    member
        .update(
            db,
            PartialMember {
                roles: Some(roles),
                ..Default::default()
            },
            vec![],
        )
        .await?;

    if let Database::MongoDb(mongo) = &**db {
        mongo
            .col::<Document>("server_onboarding_completions")
            .update_one(
                doc! {
                    "_id.server": &server.id,
                    "_id.user": &user.id,
                },
                doc! { "$set": { "completed": true } },
            )
            .upsert(true)
            .await
            .map_err(|_| {
                create_database_error!("update_one", "server_onboarding_completions")
            })?;
    }

    Ok(Json(CompleteOnboardingResponse {
        roles: selected_roles,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(multiple: bool) -> OnboardingQuestion {
        OnboardingQuestion {
            id: "platform".into(),
            prompt: "What do you use?".into(),
            multiple,
            options: vec![
                OnboardingOption {
                    id: "desktop".into(),
                    label: "Desktop".into(),
                    role_id: "role-desktop".into(),
                },
                OnboardingOption {
                    id: "mobile".into(),
                    label: "Mobile".into(),
                    role_id: "role-mobile".into(),
                },
            ],
        }
    }

    #[test]
    fn validates_role_targets_and_question_ids() {
        let questions = vec![question(false)];
        let role_ids = ["role-desktop", "role-mobile"]
            .iter()
            .map(|role_id| role_id.to_string())
            .collect();

        assert!(validate_question_config(&questions, &role_ids, "server").is_ok());

        let mut invalid = questions.clone();
        invalid[0].options[0].role_id = "missing-role".into();
        assert!(validate_question_config(&invalid, &role_ids, "server").is_err());

        let mut duplicate = questions;
        duplicate.push(question(false));
        assert!(validate_question_config(&duplicate, &role_ids, "server").is_err());
    }

    #[test]
    fn selects_roles_for_valid_answers_in_stable_order() {
        let questions = vec![question(true)];
        let answers = HashMap::from([("platform".into(), vec!["mobile".into(), "desktop".into()])]);

        assert_eq!(
            selected_roles(&questions, answers).unwrap(),
            vec!["role-desktop", "role-mobile"]
        );
    }

    #[test]
    fn rejects_unknown_and_multiple_answers_for_single_choice() {
        let questions = vec![question(false)];
        let too_many =
            HashMap::from([("platform".into(), vec!["desktop".into(), "mobile".into()])]);
        assert!(selected_roles(&questions, too_many).is_err());

        let unknown_question = HashMap::from([("unknown".into(), vec!["desktop".into()])]);
        assert!(selected_roles(&questions, unknown_question).is_err());

        let unknown_option = HashMap::from([("platform".into(), vec!["unknown".into()])]);
        assert!(selected_roles(&questions, unknown_option).is_err());
    }
}
