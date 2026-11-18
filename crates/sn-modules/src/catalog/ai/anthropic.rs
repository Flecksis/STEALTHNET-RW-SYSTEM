use super::llm::{ChatMessage, ChatOptions, Completion, Fault, answer_schema, cut, tokens};
use serde::Deserialize;
use serde_json::{Value, json};

pub const VERSION: &str = "2026-10-21";

pub fn body(model: &str, messages: &[ChatMessage], options: ChatOptions) -> Value {
    let system: Vec<&str> = messages
        .iter()
        .filter(|m| m.role == "system")
        .map(|m| m.content.as_str())
        .collect();
    let mut turns: Vec<(&'static str, String)> = Vec::new();
    for message in messages.iter().filter(|m| m.role != "system") {
        match turns.last_mut() {
            Some((role, text)) if *role == message.role => {
                text.push_str("\n\n");
                text.push_str(&message.content);
            }
            _ => turns.push((message.role, message.content.clone())),
        }
    }
    while turns.first().is_some_and(|(role, _)| *role == "assistant") {
        turns.remove(0);
    }
    while turns.last().is_some_and(|(role, _)| *role == "assistant") {
        turns.pop();
    }
    let mut body = serde_json::Map::new();
    body.insert("model".into(), json!(model));
    body.insert("max_tokens".into(), json!(options.max_tokens));
    if !system.is_empty() {
        body.insert("system".into(), json!(system.join("\n\n")));
    }
    body.insert(
        "messages".into(),
        Value::Array(
            turns
                .into_iter()
                .map(|(role, content)| json!({ "role": role, "content": content }))
                .collect(),
        ),
    );
    let mut output = serde_json::Map::new();
    if options.reasoning {
        output.insert("effort".into(), json!("low"));
    }
    if options.json {
        output.insert(
            "format".into(),
            json!({ "type": "json_schema", "schema": answer_schema() }),
        );
    }
    if !output.is_empty() {
        body.insert("output_config".into(), Value::Object(output));
    }
    Value::Object(body)
}

#[derive(Deserialize)]
struct MessageResponse {
    #[serde(default)]
    content: Vec<Block>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<MessageUsage>,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize, Default)]
struct MessageUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
}

pub fn completion(body: &str) -> Result<Completion, Fault> {
    let parsed: MessageResponse =
        serde_json::from_str(body).map_err(|e| Fault::Malformed(cut(&e.to_string())))?;
    let content: String = parsed
        .content
        .iter()
        .filter(|block| block.kind == "text")
        .filter_map(|block| block.text.as_deref())
        .collect();
    let usage = parsed.usage.unwrap_or_default();
    let prompt = usage.input_tokens
        + usage.cache_creation_input_tokens.unwrap_or(0)
        + usage.cache_read_input_tokens.unwrap_or(0);
    Ok(Completion {
        content,
        finish_reason: parsed.stop_reason.map(|reason| match reason.as_str() {
            "max_tokens" => "length".to_string(),
            _ => reason,
        }),
        prompt_tokens: tokens(prompt),
        completion_tokens: tokens(usage.output_tokens),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::ai::llm::Param;
    use crate::catalog::ai::provider::Provider;

    #[test]
    fn запрос_к_anthropic_собирается_по_messages_api() {
        let messages = [
            ChatMessage::system("правила"),
            ChatMessage::assistant("старый ответ"),
            ChatMessage::user("первое"),
            ChatMessage::user("второе"),
            ChatMessage::assistant("{\"reply\":\"ответ\"}"),
            ChatMessage::user("ещё вопрос"),
        ];
        let options = ChatOptions::full(900, Provider::Anthropic, "claude-haiku-5-5");
        let request = body("claude-haiku-5-5", &messages, options);
        assert_eq!(request["model"], "claude-haiku-5-5");
        assert_eq!(request["max_tokens"], 900);
        assert_eq!(request["system"], "правила");
        assert_eq!(
            request["messages"],
            json!([
                { "role": "user", "content": "первое\n\nвторое" },
                { "role": "assistant", "content": "{\"reply\":\"ответ\"}" },
                { "role": "user", "content": "ещё вопрос" }
            ])
        );
        assert_eq!(request["output_config"]["effort"], "low");
        assert_eq!(request["output_config"]["format"]["type"], "json_schema");
        assert_eq!(
            request["output_config"]["format"]["schema"]["additionalProperties"],
            false
        );
        for key in ["temperature", "top_p", "thinking", "response_format"] {
            assert!(request.get(key).is_none(), "{key}");
        }
        let bare = body(
            "claude-haiku-4-5",
            &messages,
            options.without(Param::Reasoning).without(Param::Json),
        );
        assert!(bare.get("output_config").is_none());
    }

    #[test]
    fn ответ_anthropic_разбирается() {
        let done = completion(
            r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-haiku-5-5","content":[{"type":"thinking","thinking":"","signature":"sig"},{"type":"text","text":"{\"reply\":"},{"type":"text","text":"\"Здравствуйте\",\"handoff\":false,\"reason\":\"\"}"}],"stop_reason":"end_turn","usage":{"input_tokens":1200,"output_tokens":45,"cache_creation_input_tokens":0,"cache_read_input_tokens":300}}"#,
        )
        .unwrap();
        assert_eq!(
            done.content,
            r#"{"reply":"Здравствуйте","handoff":false,"reason":""}"#
        );
        assert_eq!(done.finish_reason.as_deref(), Some("end_turn"));
        assert_eq!((done.prompt_tokens, done.completion_tokens), (1_500, 45));
        let cut_off = completion(
            r#"{"content":[{"type":"text","text":"{\"reply\":\"Здр"}],"stop_reason":"max_tokens","usage":{"input_tokens":10,"output_tokens":900}}"#,
        )
        .unwrap();
        assert_eq!(cut_off.finish_reason.as_deref(), Some("length"));
        assert!(matches!(completion("not json"), Err(Fault::Malformed(_))));
    }
}
