//! Anthropic provider 的 HTTP/SSE 集成测试（wiremock 本地 mock）。

use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, ResponseTemplate};

use mrorcode_capabilities::provider::{AnthropicConfig, AnthropicProvider};
use mrorcode_kernel::message::Message;
use mrorcode_kernel::provider::{ChatOptions, LlmProvider, StreamEvent};
use mrorcode_kernel::tool::ToolDef;

fn provider_for(base_url: &str) -> AnthropicProvider {
    AnthropicProvider::new(AnthropicConfig {
        base_url: base_url.to_string(),
        api_key: "test-key".to_string(),
        model: "claude-test".to_string(),
        context_window: 8192,
        timeout: Duration::from_secs(10),
        max_tokens: 4096,
    })
    .unwrap()
}

/// 构造 Anthropic SSE 流：每条 data 一行。
fn sse_body(events: &[&str]) -> String {
    let mut s = String::new();
    for e in events {
        s.push_str("data: ");
        s.push_str(e);
        s.push_str("\n\n");
    }
    s
}

#[tokio::test]
async fn streams_text_chunks() {
    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("cache-control", "no-cache")
                .set_body_string(sse_body(&[
                    r#"{"type":"message_start","message":{"usage":{"input_tokens":10}}}"#,
                    r#"{"type":"content_block_start","index":0,"block":{"type":"text"}}"#,
                    r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}}"#,
                    r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"lo"}}"#,
                    r#"{"type":"content_block_stop","index":0}"#,
                    r#"{"type":"message_delta","usage":{"output_tokens":5}}"#,
                    r#"{"type":"message_stop"}"#,
                ])),
        )
        .mount(&server)
        .await;

    let provider = provider_for(&server.uri());
    let cancel = CancellationToken::new();
    let options = ChatOptions::default();
    let messages = vec![Message::user("hi")];
    let events: Vec<StreamEvent> = provider
        .chat_stream(&messages, &[], &options, &cancel)
        .await
        .unwrap()
        .collect()
        .await;

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Hello");

    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::Usage {
            input_tokens: 10,
            output_tokens: 0
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::Usage {
            input_tokens: 0,
            output_tokens: 5
        }
    )));
}

#[tokio::test]
async fn streams_tool_use_roundtrip() {
    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("cache-control", "no-cache")
                .set_body_string(sse_body(&[
                    r#"{"type":"message_start","message":{"usage":{"input_tokens":5}}}"#,
                    r#"{"type":"content_block_start","index":0,"block":{"type":"tool_use","id":"toolu_1","name":"bash"}}"#,
                    r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"cmd\":"}}"#,
                    r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"\"ls\"}"}}"#,
                    r#"{"type":"content_block_stop","index":0}"#,
                    r#"{"type":"message_delta","usage":{"output_tokens":3}}"#,
                    r#"{"type":"message_stop"}"#,
                ])),
        )
        .mount(&server)
        .await;

    let provider = provider_for(&server.uri());
    let tools = vec![ToolDef {
        name: "bash".into(),
        description: "run".into(),
        parameters: json!({"type": "object"}),
    }];
    let cancel = CancellationToken::new();
    let options = ChatOptions::default();
    let messages = vec![Message::user("go")];
    let events: Vec<StreamEvent> = provider
        .chat_stream(&messages, &tools, &options, &cancel)
        .await
        .unwrap()
        .collect()
        .await;

    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::ToolCallStarted { id, name } if id == "toolu_1" && name == "bash"
    )));
    let args: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolCallDelta { id, arguments } if id == "toolu_1" => {
                Some(arguments.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(args, r#"{"cmd":"ls"}"#);
}

#[tokio::test]
async fn overloaded_error_is_retryable() {
    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[
                    r#"{"type":"error","error":{"type":"overloaded_error","message":"server busy"}}"#,
                ])),
        )
        .mount(&server)
        .await;

    let provider = provider_for(&server.uri());
    let cancel = CancellationToken::new();
    let options = ChatOptions::default();
    let messages = vec![Message::user("hi")];
    let events: Vec<StreamEvent> = provider
        .chat_stream(&messages, &[], &options, &cancel)
        .await
        .unwrap()
        .collect()
        .await;

    match &events[0] {
        StreamEvent::Error(e) => {
            assert!(e.retryable);
            assert_eq!(e.message, "server busy");
        }
        other => panic!("expected Error event, got {other:?}"),
    }
}
