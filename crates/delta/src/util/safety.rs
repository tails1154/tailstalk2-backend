//! AI safety review of new messages from school accounts.
//!
//! Scope: only accounts whose verified email ends in `@cats.angletonisd.net`. Every other message
//! is skipped before any AI call. Flags go to the staff review queue. The scanner never warns,
//! suspends, or bans anyone itself, and nothing is emailed.

use std::sync::OnceLock;
use std::time::Duration;

use revolt_config::{config, ApiAisd};
use revolt_database::mongodb::bson::{self, doc};
use futures::StreamExt;
use revolt_database::{Database, EmailVerification, Message};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Semaphore;

const SCHOOL_DOMAIN: &str = "@cats.angletonisd.net";

const SYSTEM_PROMPT: &str = "You review chat messages for a school platform. Decide whether the message \
expresses a real intention to harm oneself (self_harm) or a real threat of violence against another person (threat). \
Quoted text, fiction, jokes, song lyrics, or discussion of a topic without intent do not count. \
Earlier messages from the channel are given for context only. Judge only the message marked as the message to judge, \
and use the earlier messages to work out what words like \"it\", \"this\" or \"do it\" refer to. \
Sarcasm and exaggeration are not intent, and a joke marker means it is not a real threat: words like \"trust\", \"lol\", \"jk\", \"/s\" or \"haha\" at the end usually mean sarcasm. \
For example \"im gonna execute you trust\" is sarcasm about a company or a product, not a threat, so do not flag it. \
A message that continues a plan stated earlier counts as intent. \
Short strings are often not self-harm. \"sh\" alone means nothing: it can be shh, a typo for ssh, or the shell sh. \
Only flag \"sh\" when the message says the sender plans to self-harm, for example \"gonna do sh\" or \"gonna do sh to myself\". \
A stated plan still counts if the sender adds that it is a test or that they are not really considering it: flag it and let staff decide. \
Reply with only a JSON object: {\"self_harm\": true or false, \"threat\": true or false, \"confidence\": a number from 0 to 1, \"reason\": one short sentence saying why, quoting the key words}.";

#[derive(Debug, Deserialize)]
struct Verdict {
    #[serde(default)]
    self_harm: bool,
    #[serde(default)]
    threat: bool,
    #[serde(default)]
    confidence: f32,
    /// One sentence from the model saying why it decided this
    #[serde(default)]
    reason: String,
}

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("valid reqwest client")
    })
}

/// Limits how many llama-server requests run at once
fn limit() -> &'static Semaphore {
    static LIMIT: OnceLock<Semaphore> = OnceLock::new();
    LIMIT.get_or_init(|| Semaphore::new(4))
}

/// Whether a verified account belongs to the school scope
pub fn in_scope(email: &str) -> bool {
    email.to_lowercase().ends_with(SCHOOL_DOMAIN)
}

/// Whether an email may open the staff panel. Only angletonisd.net addresses and the configured testing addresses.
pub fn panel_access_allowed(email: &str, aisd: &ApiAisd) -> bool {
    let email = email.trim().to_lowercase();
    let Some((_, domain)) = email.rsplit_once('@') else {
        return false;
    };
    aisd.panel_allowed_domains.iter().any(|d| d.to_lowercase() == domain)
        || aisd.panel_allowed_emails.iter().any(|e| e.to_lowercase() == email)
}

/// Removes llama-server thinking blocks so only the final answer is parsed
fn strip_thinking(text: &str) -> String {
    let mut out = text.to_string();
    while let (Some(start), Some(end)) = (out.find("<think>"), out.find("</think>")) {
        if end < start {
            break;
        }
        out.replace_range(start..end + "</think>".len(), "");
    }
    out
}

/// Pulls the JSON object out of the model's reply, even if it adds prose or code fences
fn parse_verdict(text: &str) -> Result<Verdict, String> {
    let text = strip_thinking(text);
    let start = text.find('{').ok_or("no JSON object in reply")?;
    let end = text.rfind('}').ok_or("no JSON object in reply")?;
    serde_json::from_str(&text[start..=end]).map_err(|e| e.to_string())
}

/// Returns the verdict and the model's own reasoning trace, when llama-server includes one
async fn classify(aisd: &ApiAisd, content: &str) -> Result<(Verdict, String), String> {
    let url = format!("{}/v1/chat/completions", aisd.url.trim_end_matches('/'));
    let body = json!({
        "model": aisd.model,
        "temperature": 0,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": content },
        ],
    });

    let mut request = http().post(url).json(&body);
    if !aisd.api_key.is_empty() {
        request = request.bearer_auth(&aisd.api_key);
    }

    let response: serde_json::Value = request
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let text = response["choices"][0]["message"]["content"]
        .as_str()
        .ok_or("reply has no message content")?;
    let reasoning = response["choices"][0]["message"]["reasoning_content"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    Ok((parse_verdict(text)?, reasoning))
}

/// The last few messages in the channel before this one, oldest first, so the model can read what "it" refers to
async fn earlier_messages(db: &Database, message: &Message, author_id: &str) -> String {
    let Database::MongoDb(mongo) = db else {
        return String::new();
    };
    let Ok(mut cursor) = mongo
        .col::<bson::Document>("messages")
        .find(doc! { "channel": &message.channel, "_id": { "$lt": &message.id } })
        .sort(doc! { "_id": -1 })
        .limit(5)
        .await
    else {
        return String::new();
    };
    let mut lines = Vec::new();
    while let Some(Ok(d)) = cursor.next().await {
        let content = d.get_str("content").unwrap_or_default();
        if content.trim().is_empty() {
            continue;
        }
        let who = if d.get_str("author").unwrap_or_default() == author_id {
            "sender"
        } else {
            "other person"
        };
        let text: String = content.chars().take(300).collect();
        lines.push(format!("{who}: {text}"));
    }
    lines.reverse();
    lines.join("\n")
}

/// Runs the review in the background so sending a message is never delayed or blocked by it
pub fn spawn_check(db: Database, message: Message, author_id: String) {
    tokio::spawn(async move {
        if let Err(err) = check(&db, &message, &author_id).await {
            log::error!("Safety review failed for message {}: {err}", message.id);
        }
    });
}

async fn check(db: &Database, message: &Message, author_id: &str) -> Result<(), String> {
    let settings = config().await.api.aisd;
    if !settings.enabled {
        return Ok(());
    }

    let account = db.fetch_account(author_id).await.map_err(|e| format!("{e:?}"))?;
    if !matches!(account.verification, EmailVerification::Verified) || !in_scope(&account.email) {
        return Ok(());
    }

    let Some(content) = message.content.as_deref().filter(|c| !c.trim().is_empty()) else {
        return Ok(());
    };

    let _permit = limit().acquire().await.map_err(|e| e.to_string())?;
    let earlier = earlier_messages(db, message, author_id).await;
    let judged = if earlier.is_empty() {
        content.to_string()
    } else {
        format!("Earlier messages in this channel:\n{earlier}\n\nMessage to judge (from the sender):\n{content}")
    };
    let (verdict, reasoning) = classify(&settings, &judged).await?;

    let kind = match (verdict.self_harm, verdict.threat) {
        (true, true) => "self_harm, threat",
        (true, false) => "self_harm",
        (false, true) => "threat",
        (false, false) => return Ok(()),
    };
    if verdict.confidence < settings.threshold {
        return Ok(());
    }

    // Flags are shown as notifications in the AISD panel. Nothing is emailed.
    store_flag(db, message, author_id, kind, verdict.confidence, &verdict.reason, &reasoning).await;
    Ok(())
}

async fn store_flag(
    db: &Database,
    message: &Message,
    author_id: &str,
    kind: &str,
    confidence: f32,
    reason: &str,
    reasoning: &str,
) {
    let Database::MongoDb(mongo) = db else {
        return;
    };
    let result = mongo
        .col::<bson::Document>("safety_flags")
        .insert_one(doc! {
            "kind": kind,
            "confidence": confidence as f64,
            "reason": reason,
            "reasoning": reasoning.chars().take(8000).collect::<String>(),
            "actions": Vec::<bson::Bson>::new(),
            "message_id": &message.id,
            "channel_id": &message.channel,
            "author_id": author_id,
            "created_at": iso8601_timestamp::Timestamp::now_utc().to_string(),
            "reviewed": false,
        })
        .await;
    if let Err(err) = result {
        log::error!("Failed to store safety flag for message {}: {err:?}", message.id);
    }
}
