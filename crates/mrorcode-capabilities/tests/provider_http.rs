//! OpenAI 兼容 provider 的 HTTP/SSE 集成测试（wiremock 本地 mock）。

use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, ResponseTemplate};

use mrorcode_capabilities::provider::{OpenAiCompatConfig, OpenAiCompatProvider};
use mrorcode_kernel::message::Message;
use mrorcode_kernel::provider::{ChatOptions, LlmProvider, StreamEvent};
use mrorcode_kernel::tool::ToolDef;

fn provider_for(base_url: &str) -> OpenAiCompatProvider {
    OpenAiCompatProvider::new(OpenAiCompatConfig {
        base_url: format!("{base_url}/v1"),
        api_key: "test-key".to_string(),
        model: "test-model".to_string(),
        context_window: 8192,
        timeout: Duration::from_secs(10),
    })
    .unwrap()
}

fn sse_body(chunks: &[&str]) -> String {
    let mut s = String::new();
    for c in chunks {
        s.push_str("data: ");
        s.push_str(c);
        s.push_str("\n\n");
    }
    s.push_str("data: [DONE]\n\n");
    s
}

#[tokio::test]
async fn streams_text_chunks() {
    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("cache-control", "no-cache")
                .set_body_string(sse_body(&[
                    r#"{"choices":[{"delta":{"content":"Hel"}}]}"#,
                    r#"{"choices":[{"delta":{"content":"lo"}}]}"#,
                ])),
        )
        .mount(&server)
        .await;

    let provider = provider_for(&server.uri());
    let cancel = CancellationToken::new();
    let options = ChatOptions::default();
    let messages = vec![Message::user("hi")];
    let stream = provider
        .chat_stream(&messages, &[], &options, &cancel)
        .await
        .expect("stream starts");
    let events: Vec<StreamEvent> = stream.collect().await;

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Hello");
}

#[tokio::test]
async fn streams_tool_call_arguments() {
    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream").insert_header("cache-control", "no-cache")
                .set_body_string(sse_body(&[
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"bash","arguments":""}}]}}]}"#,
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"cmd\":\"ls\"}"}}]}}]}"#,
                ])),
        )
        .mount(&server)
        .await;

    let provider = provider_for(&server.uri());
    let tools = vec![ToolDef {
        name: "bash".into(),
        description: "run".into(),
        parameters: json!({"type":"object"}),
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
        StreamEvent::ToolCallStarted { id, name } if id == "c1" && name == "bash"
    )));
    let args: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolCallDelta { id, arguments } if id == "c1" => Some(arguments.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(args, r#"{"cmd":"ls"}"#);
}

#[tokio::test]
async fn retries_after_503_then_succeeds() {
    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(503).insert_header("retry-after", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("cache-control", "no-cache")
                .set_body_string(sse_body(&[r#"{"choices":[{"delta":{"content":"ok"}}]}"#])),
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
        .expect("retry succeeds")
        .collect()
        .await;
    assert!(events
        .iter()
        .any(|e| matches!(e, StreamEvent::TextDelta(t) if t == "ok")));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn unauthorized_is_not_retried() {
    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
        .mount(&server)
        .await;

    let provider = provider_for(&server.uri());
    let cancel = CancellationToken::new();
    let options = ChatOptions::default();
    let messages = vec![Message::user("hi")];
    let err = match provider
        .chat_stream(&messages, &[], &options, &cancel)
        .await
    {
        Err(e) => e,
        Ok(_) => panic!("401 should be an error"),
    };
    assert_eq!(err.http_status, Some(401));
    assert!(!err.retryable);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
