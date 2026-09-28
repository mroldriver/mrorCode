//! M5-d：openai_compat 与 anthropic 的 provider conformance。
//!
//! 两个 wire 格式完全不同（chunk vs SSE event），但必须归一化成**同一份**
//! `StreamEvent` 序列——这是 agent 唯一看到的契约。此文件锁住它：
//! 任何一个 provider 改了归一化逻辑，另一个的等价响应就对不上。

use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use mrorcode_capabilities::provider::{
    AnthropicConfig, AnthropicProvider, OpenAiCompatConfig, OpenAiCompatProvider,
};
use mrorcode_kernel::message::Message;
use mrorcode_kernel::provider::{ChatOptions, LlmProvider, StreamEvent};
use mrorcode_kernel::testkit::{assert_stream_sequence, stream_signature};
use mrorcode_kernel::tool::ToolDef;

fn sse_body(events: &[&str]) -> String {
    let mut s = String::new();
    for e in events {
        s.push_str("data: ");
        s.push_str(e);
        s.push_str("\n\n");
    }
    s
}

fn stream_of(body_events: &[&str]) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .insert_header("cache-control", "no-cache")
        .set_body_string(sse_body(body_events))
}

fn openai(base_url: &str) -> OpenAiCompatProvider {
    OpenAiCompatProvider::new(OpenAiCompatConfig {
        base_url: base_url.to_string(),
        api_key: "test-key".to_string(),
        model: "gpt-test".to_string(),
        context_window: 8192,
        timeout: Duration::from_secs(10),
    })
    .unwrap()
}

fn anthropic(base_url: &str) -> AnthropicProvider {
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

fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "bash".into(),
        description: "run".into(),
        parameters: json!({"type": "object"}),
    }]
}

async fn drain(provider: &dyn LlmProvider, tools: &[ToolDef]) -> Vec<StreamEvent> {
    drain_with(provider, &[Message::user("hi")], tools).await
}

async fn drain_with(
    provider: &dyn LlmProvider,
    messages: &[Message],
    tools: &[ToolDef],
) -> Vec<StreamEvent> {
    provider
        .chat_stream(
            messages,
            tools,
            &ChatOptions::default(),
            &CancellationToken::new(),
        )
        .await
        .expect("stream opens")
        .collect()
        .await
}

fn sum_usage(evs: &[StreamEvent]) -> (u64, u64) {
    evs.iter().fold((0, 0), |(i, o), e| match e {
        StreamEvent::Usage {
            input_tokens,
            output_tokens,
        } => (i + input_tokens, o + output_tokens),
        _ => (i, o),
    })
}

fn non_usage_signatures(evs: &[StreamEvent]) -> Vec<String> {
    evs.iter()
        .filter(|e| !matches!(e, StreamEvent::Usage { .. }))
        .map(stream_signature)
        .collect()
}

/// 同一语义响应的两种线缆：文本 "Hello" + 一次 `bash` 调用 + usage(11/4)。
async fn openai_equivalent() -> Vec<StreamEvent> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(stream_of(&[
            r#"{"choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"{"choices":[{"delta":{"content":"lo"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"bash","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"command\":\"ls\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":11,"completion_tokens":4}}"#,
        ]))
        .mount(&server)
        .await;
    drain(&openai(&server.uri()), &tools()).await
}

async fn anthropic_equivalent() -> Vec<StreamEvent> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(stream_of(&[
            r#"{"type":"message_start","message":{"usage":{"input_tokens":11}}}"#,
            r#"{"type":"content_block_start","index":0,"block":{"type":"text"}}"#,
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}}"#,
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"lo"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"block":{"type":"tool_use","id":"call_1","name":"bash"}}"#,
            r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"command\":\"ls\"}"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"message_delta","usage":{"output_tokens":4}}"#,
            r#"{"type":"message_stop"}"#,
        ]))
        .mount(&server)
        .await;
    drain(&anthropic(&server.uri()), &tools()).await
}

/// 两个 provider 的等价响应必须归一化成同一份非 usage 序列，usage 求和后相等。
#[tokio::test]
async fn equivalent_responses_normalize_to_the_same_sequence() {
    let oa = openai_equivalent().await;
    let an = anthropic_equivalent().await;

    assert_eq!(
        non_usage_signatures(&oa),
        non_usage_signatures(&an),
        "两个 provider 的非 usage 事件序列必须一致"
    );
    // usage 分片方式不同（openai 一次给全，anthropic 头尾分两次），求和后相等
    assert_eq!(sum_usage(&oa), (11, 4), "openai usage");
    assert_eq!(sum_usage(&an), (11, 4), "anthropic usage");

    // 各自锁定完整序列（防止只改一边）
    assert_stream_sequence(
        &oa,
        &[
            "text:Hel",
            "text:lo",
            "tool_start:call_1:bash",
            r#"tool_args:call_1:{"command":"ls"}"#,
            "usage:11/4",
        ],
    );
    assert_stream_sequence(
        &an,
        &[
            "usage:11/0",
            "text:Hel",
            "text:lo",
            "tool_start:call_1:bash",
            r#"tool_args:call_1:{"command":"ls"}"#,
            "usage:0/4",
        ],
    );
}

/// 流错误在两个 provider 上都归一化成 StreamEvent::Error。
#[tokio::test]
async fn stream_errors_normalize_the_same_way() {
    let oa_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(stream_of(&[
            r#"{"error":{"message":"boom","code":"oops"}}"#,
        ]))
        .mount(&oa_server)
        .await;
    let oa = drain(&openai(&oa_server.uri()), &[]).await;
    let oa_err = oa.iter().find_map(|e| match e {
        StreamEvent::Error(e) => Some(e.clone()),
        _ => None,
    });
    assert!(
        oa_err.as_ref().is_some_and(|e| e.message == "boom"),
        "openai 未归一化成 Error：{oa:?}"
    );

    let an_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(stream_of(&[
            r#"{"type":"error","error":{"type":"overloaded_error","message":"boom"}}"#,
        ]))
        .mount(&an_server)
        .await;
    let an = drain(&anthropic(&an_server.uri()), &[]).await;
    let an_err = an.iter().find_map(|e| match e {
        StreamEvent::Error(e) => Some(e.clone()),
        _ => None,
    });
    assert!(
        an_err.as_ref().is_some_and(|e| e.message == "boom"),
        "anthropic 未归一化成 Error：{an:?}"
    );
    // 两边 message 一致（wire 上的 code/type 差异被吃掉）
    assert_eq!(oa_err.unwrap().message, an_err.unwrap().message);
}

/// 请求体形状：messages/tools 语义一致，system 的摆放是协议差异（不算漂移）。
#[tokio::test]
async fn request_bodies_carry_equivalent_semantics() {
    let oa_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(stream_of(&[r#"{"choices":[{"delta":{"content":"x"}}]}"#]))
        .expect(1)
        .mount(&oa_server)
        .await;
    let _ = drain(&openai(&oa_server.uri()), &tools()).await;
    let oa_body = oa_server.received_requests().await.unwrap();
    let oa_json: serde_json::Value =
        serde_json::from_slice(&oa_body[0].body).expect("openai body is json");
    assert_eq!(oa_json["model"], "gpt-test");
    assert_eq!(oa_json["messages"][0]["role"], "user");
    assert_eq!(oa_json["messages"][0]["content"], "hi");
    assert_eq!(oa_json["tools"][0]["function"]["name"], "bash");

    let an_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(stream_of(&[r#"{"type":"message_stop"}"#]))
        .expect(1)
        .mount(&an_server)
        .await;
    let _ = drain(&anthropic(&an_server.uri()), &tools()).await;
    let an_body = an_server.received_requests().await.unwrap();
    let an_json: serde_json::Value =
        serde_json::from_slice(&an_body[0].body).expect("anthropic body is json");
    assert_eq!(an_json["model"], "claude-test");
    assert_eq!(an_json["messages"][0]["role"], "user");
    // 协议差异（不算漂移）：anthropic 用 content block 数组，openai 用纯字符串
    assert_eq!(an_json["messages"][0]["content"][0]["type"], "text");
    assert_eq!(an_json["messages"][0]["content"][0]["text"], "hi");
    assert_eq!(an_json["tools"][0]["name"], "bash");
    // 没有 system 消息时 anthropic 也不带 system（真实 API 会 400——生产装配必带 system_prompt，
    // 下一条用例验证转发）
    assert!(an_json.get("system").is_none(), "anthropic: {an_json}");
}

/// system 消息必须转发：anthropic 把它提到顶层（必填），openai 作为首条 message。
#[tokio::test]
async fn system_message_is_forwarded_by_both_providers() {
    let oa_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(stream_of(&[r#"{"choices":[{"delta":{"content":"x"}}]}"#]))
        .expect(1)
        .mount(&oa_server)
        .await;
    let _ = drain_with(
        &openai(&oa_server.uri()),
        &[Message::system("be nice"), Message::user("hi")],
        &[],
    )
    .await;
    let reqs = oa_server.received_requests().await.unwrap();
    let oa_json: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(oa_json["messages"][0]["role"], "system");
    assert_eq!(oa_json["messages"][0]["content"], "be nice");

    let an_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(stream_of(&[r#"{"type":"message_stop"}"#]))
        .expect(1)
        .mount(&an_server)
        .await;
    let _ = drain_with(
        &anthropic(&an_server.uri()),
        &[Message::system("be nice"), Message::user("hi")],
        &[],
    )
    .await;
    let reqs = an_server.received_requests().await.unwrap();
    let an_json: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(an_json["system"], "be nice");
    assert!(
        an_json["messages"].as_array().unwrap().len() == 1,
        "system 不进 messages"
    );
}
