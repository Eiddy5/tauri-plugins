use serde_json::{json, Value};
use std::sync::Arc;
use tauri_plugin_ai::{
    BridgeEvent, Caller, CapabilityRuntime, Completion, McpConfig, McpServer, ToolDefinition,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn definition(timeout_ms: u64) -> ToolDefinition {
    serde_json::from_value(json!({
        "name":"test.echo","description":"Echo","policy":{"timeoutMs":timeout_ms},"annotations":{"readOnlyHint":true},
        "inputSchema":{"type":"object","properties":{"value":{"type":"string"}},"required":["value"],"additionalProperties":false},
        "outputSchema":{"type":"object","properties":{"value":{"type":"string"}},"required":["value"],"additionalProperties":false}
    })).unwrap()
}

fn connected(
    timeout_ms: u64,
) -> (
    CapabilityRuntime,
    String,
    mpsc::UnboundedReceiver<BridgeEvent>,
) {
    let runtime = CapabilityRuntime::default();
    let (sender, receiver) = mpsc::unbounded_channel();
    let id = runtime
        .connect(
            "trusted".into(),
            vec![definition(timeout_ms)],
            Arc::new(move |event| {
                sender
                    .send(event)
                    .map_err(|_| tauri_plugin_ai::Error::new("NOT_READY", "Closed"))
            }),
        )
        .unwrap();
    runtime.ready("trusted", &id).unwrap();
    (runtime, id, receiver)
}

async fn request(receiver: &mut mpsc::UnboundedReceiver<BridgeEvent>) -> String {
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(3), receiver.recv())
            .await
            .unwrap()
            .unwrap()
        {
            BridgeEvent::Call { request_id, .. } => return request_id,
            BridgeEvent::State { .. } => {}
            other => panic!("expected call, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn dispatch_result_validates_source_and_completes_once() {
    let (runtime, session, mut events) = connected(2000);
    let caller = runtime.clone();
    let call = tokio::spawn(async move {
        caller
            .call(
                Caller::local("trusted"),
                "test.echo",
                json!({"value":"hello"}),
                CancellationToken::new(),
            )
            .await
    });
    let id = request(&mut events).await;
    let result = Completion::Success {
        result: json!({"value":"hello"}),
    };
    assert_eq!(
        runtime
            .resolve("other", &session, &id, result.clone())
            .unwrap_err()
            .code,
        "STALE_SESSION"
    );
    assert_eq!(runtime.pending_count(), 1);
    runtime
        .resolve("trusted", &session, &id, result.clone())
        .unwrap();
    assert_eq!(call.await.unwrap().unwrap(), json!({"value":"hello"}));
    assert_eq!(
        runtime
            .resolve("trusted", &session, &id, result)
            .unwrap_err()
            .code,
        "STALE_REQUEST"
    );
    assert_eq!(runtime.pending_count(), 0);
}

#[tokio::test]
async fn schema_errors_do_not_execute_or_escape_the_runtime() {
    let (runtime, session, mut events) = connected(2000);
    assert_eq!(
        runtime
            .call(
                Caller::local("trusted"),
                "test.echo",
                json!({"value":1}),
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .code,
        "INVALID_ARGUMENT"
    );
    assert!(
        matches!(events.try_recv().unwrap(), BridgeEvent::State { event, .. } if event.error_code.as_deref() == Some("INVALID_ARGUMENT"))
    );
    assert!(events.try_recv().is_err());
    let caller = runtime.clone();
    let call = tokio::spawn(async move {
        caller
            .call(
                Caller::local("trusted"),
                "test.echo",
                json!({"value":"hello"}),
                CancellationToken::new(),
            )
            .await
    });
    let id = request(&mut events).await;
    runtime
        .resolve(
            "trusted",
            &session,
            &id,
            Completion::Success {
                result: json!({"value":1}),
            },
        )
        .unwrap();
    assert_eq!(call.await.unwrap().unwrap_err().code, "INVALID_RESULT");
}

#[test]
fn invalid_registration_is_atomic_and_another_source_cannot_take_over() {
    let (runtime, _, _) = connected(1000);
    let sink = Arc::new(|_| Ok(()));
    assert_eq!(
        runtime
            .connect(
                "trusted".into(),
                vec![definition(1000), definition(1000)],
                sink.clone()
            )
            .unwrap_err()
            .code,
        "DUPLICATE_TOOL"
    );
    assert_eq!(runtime.list_tools(&Caller::local("trusted")).len(), 1);
    assert_eq!(
        runtime
            .connect("other".into(), vec![], sink.clone())
            .unwrap_err()
            .code,
        "BUSY"
    );
    let mut invalid = definition(1000);
    invalid.input_schema = json!({"type":"object","$ref":"file:///etc/passwd"});
    assert_eq!(
        runtime
            .connect("trusted".into(), vec![invalid], sink)
            .unwrap_err()
            .code,
        "INVALID_DEFINITION"
    );
    assert_eq!(runtime.list_tools(&Caller::local("trusted")).len(), 1);
}

#[tokio::test]
async fn timeout_sends_cancel_and_rejects_late_results() {
    let (runtime, session, mut events) = connected(25);
    let caller = runtime.clone();
    let call = tokio::spawn(async move {
        caller
            .call(
                Caller::local("trusted"),
                "test.echo",
                json!({"value":"hello"}),
                CancellationToken::new(),
            )
            .await
    });
    let id = request(&mut events).await;
    assert_eq!(call.await.unwrap().unwrap_err().code, "TIMEOUT");
    assert!(matches!(
        events.recv().await,
        Some(BridgeEvent::Cancel { .. })
    ));
    assert_eq!(runtime.pending_count(), 0);
    assert_eq!(
        runtime
            .resolve(
                "trusted",
                &session,
                &id,
                Completion::Success {
                    result: json!({"value":"late"})
                }
            )
            .unwrap_err()
            .code,
        "STALE_REQUEST"
    );
}

#[tokio::test]
async fn cancellation_and_dropped_future_release_pending() {
    let (runtime, _, mut events) = connected(2000);
    let cancellation = CancellationToken::new();
    let caller = runtime.clone();
    let token = cancellation.clone();
    let call = tokio::spawn(async move {
        caller
            .call(
                Caller::local("trusted"),
                "test.echo",
                json!({"value":"hello"}),
                token,
            )
            .await
    });
    request(&mut events).await;
    cancellation.cancel();
    assert_eq!(call.await.unwrap().unwrap_err().code, "CANCELLED");
    assert!(matches!(
        events.recv().await,
        Some(BridgeEvent::Cancel { .. })
    ));
    let caller = runtime.clone();
    let call = tokio::spawn(async move {
        caller
            .call(
                Caller::local("trusted"),
                "test.echo",
                json!({"value":"hello"}),
                CancellationToken::new(),
            )
            .await
    });
    request(&mut events).await;
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());
    assert!(matches!(
        events.recv().await,
        Some(BridgeEvent::Cancel { .. })
    ));
    assert_eq!(runtime.pending_count(), 0);
}

#[tokio::test]
async fn reconnect_revokes_old_calls_and_old_session_results() {
    let (runtime, old_session, mut events) = connected(2000);
    let caller = runtime.clone();
    let call = tokio::spawn(async move {
        caller
            .call(
                Caller::local("trusted"),
                "test.echo",
                json!({"value":"hello"}),
                CancellationToken::new(),
            )
            .await
    });
    let id = request(&mut events).await;
    let new_session = runtime
        .connect(
            "trusted".into(),
            vec![definition(2000)],
            Arc::new(|_| Ok(())),
        )
        .unwrap();
    assert_ne!(old_session, new_session);
    assert_eq!(call.await.unwrap().unwrap_err().code, "NOT_READY");
    assert_eq!(
        runtime
            .resolve(
                "trusted",
                &old_session,
                &id,
                Completion::Success {
                    result: json!({"value":"late"})
                }
            )
            .unwrap_err()
            .code,
        "STALE_SESSION"
    );
    assert_eq!(runtime.pending_count(), 0);
}

fn decode(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| {
        text.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter_map(|json| serde_json::from_str::<Value>(json).ok())
            .find(|value| value.get("result").is_some() || value.get("error").is_some())
            .expect("JSON-RPC response")
    })
}

#[tokio::test]
async fn real_http_mcp_auth_discovery_and_tool_call() {
    let (runtime, session, mut events) = connected(2000);
    let token = uuid::Uuid::new_v4().simple().to_string();
    let server = McpServer::start(runtime.clone(), McpConfig::localhost(0, token.clone())).unwrap();
    let url = format!("http://{}/mcp", server.address());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"integration-test","version":"1"}
    }});
    assert_eq!(
        client
            .post(&url)
            .json(&initialize)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .post(&url)
            .bearer_auth(&token)
            .header("Origin", "http://untrusted.invalid")
            .json(&initialize)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let initialized = client
        .post(&url)
        .bearer_auth(&token)
        .header("Accept", "application/json, text/event-stream")
        .json(&initialize)
        .send()
        .await
        .unwrap();
    assert!(initialized.status().is_success());
    let mcp_session = initialized
        .headers()
        .get("mcp-session-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let body = decode(&initialized.text().await.unwrap());
    assert_eq!(body["result"]["protocolVersion"], "2025-11-25");
    let builder = || {
        client
            .post(&url)
            .bearer_auth(&token)
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", &mcp_session)
            .header("MCP-Protocol-Version", "2025-11-25")
    };
    assert!(builder()
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    let listed = decode(
        &builder()
            .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    );
    assert_eq!(listed["result"]["tools"][0]["name"], "test.echo");
    let outgoing = builder().json(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"test.echo","arguments":{"value":"from-http"}}}));
    let response =
        tokio::spawn(async move { outgoing.send().await.unwrap().text().await.unwrap() });
    let id = request(&mut events).await;
    runtime
        .resolve(
            "trusted",
            &session,
            &id,
            Completion::Success {
                result: json!({"value":"real-http-response"}),
            },
        )
        .unwrap();
    let result = decode(&response.await.unwrap());
    assert_eq!(
        result["result"]["structuredContent"]["value"],
        "real-http-response"
    );
    assert_eq!(result["result"]["isError"], false);
    let missing = decode(&builder()
        .json(&json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"unknown","arguments":{}}}))
        .send().await.unwrap().text().await.unwrap());
    assert_eq!(missing["error"]["code"], -32602);
    assert_eq!(missing["error"]["data"]["code"], "NOT_FOUND");
    let outgoing = builder().json(&json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"test.echo","arguments":{"value":"missing-record"}}}));
    let response =
        tokio::spawn(async move { outgoing.send().await.unwrap().text().await.unwrap() });
    let id = request(&mut events).await;
    runtime
        .resolve(
            "trusted",
            &session,
            &id,
            Completion::Error {
                error: tauri_plugin_ai::Error::new("NOT_FOUND", "Business record is missing"),
            },
        )
        .unwrap();
    let result = decode(&response.await.unwrap());
    assert!(result.get("error").is_none());
    assert_eq!(result["result"]["isError"], true);
    assert_eq!(result["result"]["structuredContent"]["code"], "NOT_FOUND");
    server.shutdown();
}

#[tokio::test]
async fn capacity_is_bounded_and_disconnect_drains_all_calls() {
    let (runtime, session, mut events) = connected(5000);
    let mut calls = Vec::new();
    for _ in 0..64 {
        let caller = runtime.clone();
        calls.push(tokio::spawn(async move {
            caller
                .call(
                    Caller::local("trusted"),
                    "test.echo",
                    json!({"value":"wait"}),
                    CancellationToken::new(),
                )
                .await
        }));
        request(&mut events).await;
    }
    assert_eq!(runtime.pending_count(), 64);
    assert_eq!(
        runtime
            .call(
                Caller::local("trusted"),
                "test.echo",
                json!({"value":"overflow"}),
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .code,
        "BUSY"
    );
    runtime.disconnect("trusted", &session).unwrap();
    for call in calls {
        assert_eq!(call.await.unwrap().unwrap_err().code, "NOT_READY");
    }
    assert_eq!(runtime.pending_count(), 0);
    assert!(runtime.list_tools(&Caller::local("trusted")).is_empty());
}

#[test]
fn disconnect_cannot_deliver_cancellation_before_an_inflight_dispatch() {
    use std::sync::{mpsc as channel, Mutex};
    use std::time::Duration;
    let runtime = CapabilityRuntime::default();
    let (entered_tx, entered_rx) = channel::channel();
    let (release_tx, release_rx) = channel::channel();
    let release_rx = Mutex::new(release_rx);
    let order = Arc::new(Mutex::new(Vec::new()));
    let observed = order.clone();
    let session = runtime
        .connect(
            "trusted".into(),
            vec![definition(5000)],
            Arc::new(move |event| {
                match event {
                    BridgeEvent::Call { .. } => {
                        entered_tx.send(()).unwrap();
                        release_rx
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(3))
                            .unwrap();
                        observed.lock().unwrap().push("call");
                    }
                    BridgeEvent::Cancel { .. } => observed.lock().unwrap().push("cancel"),
                    BridgeEvent::State { .. } => {}
                }
                Ok(())
            }),
        )
        .unwrap();
    let caller = runtime.clone();
    runtime.ready("trusted", &session).unwrap();
    let calling = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(caller.call(
                Caller::local("trusted"),
                "test.echo",
                json!({"value":"race"}),
                CancellationToken::new(),
            ))
    });
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let disconnecting = std::thread::spawn(move || runtime.disconnect("trusted", &session));
    // The sink is paused before enqueueing Call. Revocation must wait for it.
    std::thread::sleep(Duration::from_millis(30));
    release_tx.send(()).unwrap();
    disconnecting.join().unwrap().unwrap();
    assert_eq!(calling.join().unwrap().unwrap_err().code, "NOT_READY");
    assert_eq!(*order.lock().unwrap(), vec!["call", "cancel"]);
}

#[tokio::test]
async fn registration_requires_ready_and_rejects_old_policy_fields() {
    let runtime = CapabilityRuntime::default();
    let id = runtime
        .connect(
            "trusted".into(),
            vec![definition(1000)],
            Arc::new(|_| Ok(())),
        )
        .unwrap();
    assert!(runtime.list_tools(&Caller::mcp()).is_empty());
    assert_eq!(
        runtime
            .call(
                Caller::mcp(),
                "test.echo",
                json!({"value":"a"}),
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .code,
        "NOT_READY"
    );
    assert_eq!(
        runtime.ready("other", &id).unwrap_err().code,
        "STALE_SESSION"
    );
    runtime.ready("trusted", &id).unwrap();
    assert_eq!(runtime.list_tools(&Caller::mcp()).len(), 1);
    for (key, value) in [
        ("readOnly", json!(true)),
        ("permission", json!("old:read")),
        ("timeoutMs", json!(100)),
    ] {
        let mut old = serde_json::to_value(definition(1000)).unwrap();
        old[key] = value;
        assert!(serde_json::from_value::<ToolDefinition>(old).is_err());
    }
}

#[tokio::test]
async fn authorization_is_shared_by_local_and_mcp_and_rejections_are_audited() {
    let mut tool = definition(1000);
    tool.policy.permissions = vec!["example:read".into()];
    let runtime = CapabilityRuntime::default();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let id = runtime
        .connect(
            "trusted".into(),
            vec![tool.clone()],
            Arc::new(move |event| {
                tx.send(event).unwrap();
                Ok(())
            }),
        )
        .unwrap();
    runtime.ready("trusted", &id).unwrap();
    for caller in [Caller::local("trusted"), Caller::mcp()] {
        assert!(runtime.list_tools(&caller).is_empty());
        assert_eq!(
            runtime
                .call(
                    caller.clone(),
                    "test.echo",
                    json!({"value":"denied"}),
                    CancellationToken::new()
                )
                .await
                .unwrap_err()
                .code,
            "UNAUTHORIZED"
        );
        assert!(
            matches!(rx.recv().await.unwrap(), BridgeEvent::State { event, pending_count: 0, .. }
            if event.error_code.as_deref() == Some("UNAUTHORIZED") && event.caller == caller)
        );
    }
    assert!(rx.try_recv().is_err());
    let runtime = CapabilityRuntime::new(tauri_plugin_ai::RuntimeConfig {
        authorizer: Some(Arc::new(|caller, tool| {
            assert_eq!(tool.policy.permissions, ["example:read"]);
            if caller.source == tauri_plugin_ai::CallSource::Local {
                Ok(())
            } else {
                Err(tauri_plugin_ai::Error::new(
                    "UNAUTHORIZED",
                    "Host denied MCP",
                ))
            }
        })),
        ..Default::default()
    })
    .unwrap();
    let id = runtime
        .connect("trusted".into(), vec![tool], Arc::new(|_| Ok(())))
        .unwrap();
    runtime.ready("trusted", &id).unwrap();
    assert_eq!(runtime.list_tools(&Caller::local("trusted")).len(), 1);
    assert!(runtime.list_tools(&Caller::mcp()).is_empty());
    assert_eq!(
        runtime
            .call(
                Caller::mcp(),
                "test.echo",
                json!({"value":"denied"}),
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
}

#[tokio::test]
async fn local_cancellation_checks_caller_ownership_and_preserves_error_code() {
    let (runtime, _, mut events) = connected(2000);
    let caller = runtime.clone();
    let call = tokio::spawn(async move {
        caller
            .call(
                Caller::local("trusted"),
                "test.echo",
                json!({"value":"waiting"}),
                CancellationToken::new(),
            )
            .await
    });
    let id = request(&mut events).await;
    assert_eq!(
        runtime.cancel_owned(&Caller::mcp(), &id).unwrap_err().code,
        "UNAUTHORIZED"
    );
    assert_eq!(runtime.pending_count(), 1);
    runtime
        .cancel_owned(&Caller::local("trusted"), &id)
        .unwrap();
    assert_eq!(call.await.unwrap().unwrap_err().code, "CANCELLED");
    assert!(
        matches!(events.recv().await.unwrap(), BridgeEvent::Cancel { error, .. } if error.code == "CANCELLED")
    );
    assert_eq!(runtime.pending_count(), 0);
}

#[tokio::test]
async fn synchronous_late_completion_is_rejected_without_waiting_for_the_timer() {
    use std::sync::Mutex;
    let runtime = CapabilityRuntime::new(tauri_plugin_ai::RuntimeConfig {
        max_timeout_ms: 20,
        ..Default::default()
    })
    .unwrap();
    let resolver = runtime.clone();
    let session = Arc::new(Mutex::new(String::new()));
    let bridge_session = session.clone();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let id = runtime
        .connect(
            "trusted".into(),
            vec![definition(2000)],
            Arc::new(move |event| {
                if let BridgeEvent::Call {
                    request_id, caller, ..
                } = &event
                {
                    assert_eq!(caller.source, tauri_plugin_ai::CallSource::Mcp);
                    std::thread::sleep(std::time::Duration::from_millis(40));
                    resolver
                        .resolve(
                            "trusted",
                            &bridge_session.lock().unwrap(),
                            request_id,
                            Completion::Success {
                                result: json!({"value":"late"}),
                            },
                        )
                        .unwrap();
                }
                tx.send(event).unwrap();
                Ok(())
            }),
        )
        .unwrap();
    *session.lock().unwrap() = id.clone();
    runtime.ready("trusted", &id).unwrap();
    assert_eq!(
        runtime
            .call(
                Caller::mcp(),
                "test.echo",
                json!({"value":"late"}),
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .code,
        "TIMEOUT"
    );
    assert_eq!(runtime.pending_count(), 0);
    let mut terminal = 0;
    let mut cancels = 0;
    while let Ok(event) = rx.try_recv() {
        match event {
            BridgeEvent::State { event, .. } if event.state == "failed" => {
                assert_eq!(event.error_code.as_deref(), Some("TIMEOUT"));
                terminal += 1;
            }
            BridgeEvent::Cancel { error, .. } => {
                assert_eq!(error.code, "TIMEOUT");
                cancels += 1;
            }
            _ => {}
        }
    }
    assert_eq!(terminal, 1);
    assert_eq!(cancels, 1);
}

#[tokio::test]
async fn per_tool_capacity_is_enforced_across_invocation_sources() {
    let mut tool = definition(2000);
    tool.policy.max_concurrency = 1;
    let runtime = CapabilityRuntime::default();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let session = runtime
        .connect(
            "trusted".into(),
            vec![tool],
            Arc::new(move |event| {
                tx.send(event).unwrap();
                Ok(())
            }),
        )
        .unwrap();
    runtime.ready("trusted", &session).unwrap();
    let caller = runtime.clone();
    let call = tokio::spawn(async move {
        caller
            .call(
                Caller::local("trusted"),
                "test.echo",
                json!({"value":"pending"}),
                CancellationToken::new(),
            )
            .await
    });
    request(&mut rx).await;
    assert_eq!(
        runtime
            .call(
                Caller::mcp(),
                "test.echo",
                json!({"value":"overflow"}),
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .code,
        "BUSY"
    );
    runtime.disconnect("trusted", &session).unwrap();
    assert_eq!(call.await.unwrap().unwrap_err().code, "NOT_READY");
}
