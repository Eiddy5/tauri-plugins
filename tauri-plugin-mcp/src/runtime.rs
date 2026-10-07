use crate::{
    registry::{self, bounded, Tool},
    BridgeEvent, Caller, Completion, Error, Result, RuntimeEvent, RuntimeSnapshot, ToolDefinition,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Enqueue events synchronously. May resolve a result, but must not reconnect
/// or disconnect synchronously from within a dispatch callback.
pub type EventSink = Arc<dyn Fn(BridgeEvent) -> Result<()> + Send + Sync>;
/// Host access rules apply to discovery and both invocation sources.
/// This callback must be bounded and must not perform blocking I/O.
pub type Authorizer = Arc<dyn Fn(&Caller, &ToolDefinition) -> Result<()> + Send + Sync>;

#[derive(Clone)]
pub struct RuntimeConfig {
    pub max_timeout_ms: u64,
    pub max_concurrency: usize,
    pub authorizer: Option<Authorizer>,
}
impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_timeout_ms: 300_000,
            max_concurrency: 64,
            authorizer: None,
        }
    }
}

struct Session {
    id: String,
    source: String,
    ready: bool,
    sink: EventSink,
    delivery: Arc<Mutex<bool>>,
}
struct Pending {
    session_id: String,
    tool: Arc<Tool>,
    caller: Caller,
    started: Instant,
    expires: Instant,
    sink: EventSink,
    delivery: Arc<Mutex<bool>>,
    sender: oneshot::Sender<Result<Value>>,
}
#[derive(Default)]
struct State {
    session: Option<Session>,
    tools: BTreeMap<String, Arc<Tool>>,
    pending: HashMap<String, Pending>,
}

/// One application runtime, shared by trusted IPC and authenticated MCP ingress.
#[derive(Clone)]
pub struct CapabilityRuntime {
    state: Arc<Mutex<State>>,
    config: Arc<RuntimeConfig>,
}
impl Default for CapabilityRuntime {
    fn default() -> Self {
        Self::new(RuntimeConfig::default()).expect("valid defaults")
    }
}

impl CapabilityRuntime {
    pub fn new(config: RuntimeConfig) -> Result<Self> {
        if !(1..=300_000).contains(&config.max_timeout_ms)
            || !(1..=64).contains(&config.max_concurrency)
        {
            return Err(Error::new(
                "INVALID_CONFIG",
                "Invalid runtime budget or capacity",
            ));
        }
        Ok(Self {
            state: Arc::new(Mutex::new(State::default())),
            config: Arc::new(config),
        })
    }

    pub fn connect(
        &self,
        source: String,
        definitions: Vec<ToolDefinition>,
        sink: EventSink,
    ) -> Result<String> {
        let tools = registry::compile(definitions)?;
        let session_id = Uuid::new_v4().to_string();
        let (previous, pending) = {
            let mut state = self.state.lock().unwrap();
            if state.session.as_ref().is_some_and(|s| s.source != source) {
                return Err(Error::new(
                    "BUSY",
                    "An application runtime is already connected",
                ));
            }
            let previous = state.session.replace(Session {
                id: session_id.clone(),
                source,
                ready: false,
                sink,
                delivery: Arc::new(Mutex::new(true)),
            });
            state.tools = tools;
            (previous, std::mem::take(&mut state.pending))
        };
        self.revoke(previous, pending);
        Ok(session_id)
    }

    fn check_session(state: &State, source: &str, session_id: &str) -> Result<()> {
        if !state
            .session
            .as_ref()
            .is_some_and(|s| s.source == source && s.id == session_id)
        {
            return Err(Error::new("STALE_SESSION", "Bridge session is not current"));
        }
        Ok(())
    }

    pub fn ready(&self, source: &str, session_id: &str) -> Result<RuntimeSnapshot> {
        {
            let mut state = self.state.lock().unwrap();
            Self::check_session(&state, source, session_id)?;
            state.session.as_mut().unwrap().ready = true;
        }
        Ok(RuntimeSnapshot {
            tools: self.list_tools(&Caller::local(source)),
            pending_count: self.pending_count(),
        })
    }

    fn revoke(&self, session: Option<Session>, pending: HashMap<String, Pending>) {
        // Preserve ordering between dispatch and revocation, even with a paused sink.
        let mut delivery = session.as_ref().map(|s| s.delivery.lock().unwrap());
        if let Some(active) = &mut delivery {
            **active = false;
        }
        for (request_id, pending) in pending {
            self.end(
                &request_id,
                pending,
                Err(Error::unknown("NOT_READY", "Runtime disconnected")),
                false,
            );
        }
    }

    pub fn disconnect(&self, source: &str, session_id: &str) -> Result<()> {
        let (session, pending) = {
            let mut state = self.state.lock().unwrap();
            Self::check_session(&state, source, session_id)?;
            state.tools.clear();
            (state.session.take(), std::mem::take(&mut state.pending))
        };
        self.revoke(session, pending);
        Ok(())
    }
    pub(crate) fn disconnect_source(&self, source: &str) {
        let id = self
            .state
            .lock()
            .unwrap()
            .session
            .as_ref()
            .filter(|s| s.source == source)
            .map(|s| s.id.clone());
        if let Some(id) = id {
            let _ = self.disconnect(source, &id);
        }
    }
    pub(crate) fn shutdown(&self) {
        let (session, pending) = {
            let mut state = self.state.lock().unwrap();
            state.tools.clear();
            (state.session.take(), std::mem::take(&mut state.pending))
        };
        self.revoke(session, pending);
    }

    fn authorize(&self, caller: &Caller, definition: &ToolDefinition) -> Result<()> {
        match &self.config.authorizer {
            Some(authorizer) => authorizer(caller, definition),
            None if definition.policy.permissions.is_empty() => Ok(()),
            None => Err(Error::new(
                "UNAUTHORIZED",
                "This tool requires a host authorizer",
            )),
        }
    }
    pub fn list_tools(&self, caller: &Caller) -> Vec<ToolDefinition> {
        let tools: Vec<_> = {
            let state = self.state.lock().unwrap();
            if !state.session.as_ref().is_some_and(|s| s.ready) {
                return Vec::new();
            }
            state.tools.values().map(|t| t.definition.clone()).collect()
        };
        tools
            .into_iter()
            .filter(|t| self.authorize(caller, t).is_ok())
            .collect()
    }
    pub fn pending_count(&self) -> usize {
        self.state.lock().unwrap().pending.len()
    }

    fn report(
        &self,
        request_id: &str,
        name: &str,
        caller: &Caller,
        started: Instant,
        status: &str,
        error: Option<&Error>,
    ) {
        let target = self
            .state
            .lock()
            .unwrap()
            .session
            .as_ref()
            .map(|s| (s.id.clone(), s.sink.clone()));
        if let Some((session_id, sink)) = target {
            let _ = sink(BridgeEvent::State {
                session_id,
                pending_count: self.pending_count(),
                event: Self::event(request_id, name, caller, started, status, error),
            });
        }
    }
    fn event(
        request_id: &str,
        name: &str,
        caller: &Caller,
        started: Instant,
        status: &str,
        error: Option<&Error>,
    ) -> RuntimeEvent {
        RuntimeEvent {
            request_id: request_id.into(),
            name: name.into(),
            source: caller.source,
            caller: caller.clone(),
            state: status.into(),
            duration_ms: started.elapsed().as_millis() as u64,
            error_code: error.map(|e| e.code.clone()),
        }
    }

    fn end(
        &self,
        request_id: &str,
        pending: Pending,
        result: Result<Value>,
        serialize_cancel: bool,
    ) {
        if let Err(error) = &result {
            if matches!(error.code.as_str(), "CANCELLED" | "TIMEOUT" | "NOT_READY") {
                let _delivery = serialize_cancel.then(|| pending.delivery.lock().unwrap());
                let _ = (pending.sink)(BridgeEvent::Cancel {
                    session_id: pending.session_id.clone(),
                    request_id: request_id.into(),
                    error: error.clone(),
                });
            }
        }
        let _ = (pending.sink)(BridgeEvent::State {
            session_id: pending.session_id.clone(),
            pending_count: self.pending_count(),
            event: Self::event(
                request_id,
                &pending.tool.definition.name,
                &pending.caller,
                pending.started,
                if result.is_ok() {
                    "completed"
                } else {
                    "failed"
                },
                result.as_ref().err(),
            ),
        });
        let _ = pending.sender.send(result);
    }

    pub fn resolve(
        &self,
        source: &str,
        session_id: &str,
        request_id: &str,
        completion: Completion,
    ) -> Result<()> {
        let (pending, result) = {
            let mut state = self.state.lock().unwrap();
            Self::check_session(&state, source, session_id)?;
            let pending = state
                .pending
                .get(request_id)
                .filter(|p| p.session_id == session_id)
                .ok_or_else(|| {
                    Error::new("STALE_REQUEST", "Request already ended or is unknown")
                })?;
            // Check before and after validation; a blocked JS loop cannot publish late success.
            let result = if Instant::now() >= pending.expires {
                Err(Self::timeout())
            } else {
                let result = bounded(&completion).and_then(|()| match completion {
                    Completion::Success { result } if pending.tool.output.is_valid(&result) => {
                        Ok(result)
                    }
                    Completion::Success { .. } => {
                        Err(Error::new("INVALID_RESULT", "Output does not match schema"))
                    }
                    Completion::Error { error } => Err(error),
                });
                if Instant::now() >= pending.expires {
                    Err(Self::timeout())
                } else {
                    result
                }
            };
            (state.pending.remove(request_id).unwrap(), result)
        };
        self.end(request_id, pending, result, false);
        Ok(())
    }

    fn timeout() -> Error {
        Error::unknown("TIMEOUT", "Tool execution timed out")
    }
    fn cancel(&self, request_id: &str, error: Error) {
        let pending = self.state.lock().unwrap().pending.remove(request_id);
        if let Some(pending) = pending {
            let error = if Instant::now() >= pending.expires {
                Self::timeout()
            } else {
                error
            };
            self.end(request_id, pending, Err(error), true);
        }
    }
    pub fn cancel_owned(&self, caller: &Caller, request_id: &str) -> Result<()> {
        let pending = {
            let mut state = self.state.lock().unwrap();
            let pending = state.pending.get(request_id).ok_or_else(|| {
                Error::new("STALE_REQUEST", "Request already ended or is unknown")
            })?;
            if &pending.caller != caller {
                return Err(Error::new(
                    "UNAUTHORIZED",
                    "Request belongs to another caller",
                ));
            }
            state.pending.remove(request_id).unwrap()
        };
        let error = if Instant::now() >= pending.expires {
            Self::timeout()
        } else {
            Error::unknown("CANCELLED", "Call cancelled")
        };
        self.end(request_id, pending, Err(error), true);
        Ok(())
    }

    pub async fn call(
        &self,
        caller: Caller,
        name: &str,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<Value> {
        self.call_with_id(
            caller,
            Uuid::new_v4().to_string(),
            name,
            arguments,
            cancellation,
        )
        .await
    }
    pub(crate) async fn invoke(
        &self,
        source: &str,
        session_id: &str,
        request_id: String,
        name: &str,
        arguments: Value,
    ) -> Result<Value> {
        Self::check_session(&self.state.lock().unwrap(), source, session_id)?;
        if Uuid::parse_str(&request_id).is_err() {
            return Err(Error::new("INVALID_ARGUMENT", "Invalid request ID"));
        }
        self.call_with_id(
            Caller::local(source),
            request_id,
            name,
            arguments,
            CancellationToken::new(),
        )
        .await
    }

    async fn call_with_id(
        &self,
        caller: Caller,
        request_id: String,
        name: &str,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<Value> {
        let started = Instant::now();
        let admitted = (|| {
            if cancellation.is_cancelled() {
                return Err(Error::new("CANCELLED", "Cancelled before dispatch"));
            }
            bounded(&arguments)?;
            let (session_id, tool) = {
                let state = self.state.lock().unwrap();
                let session = state
                    .session
                    .as_ref()
                    .filter(|s| s.ready)
                    .ok_or_else(|| Error::new("NOT_READY", "JS runtime is not ready"))?;
                let tool = state
                    .tools
                    .get(name)
                    .cloned()
                    .ok_or_else(Error::missing_tool)?;
                (session.id.clone(), tool)
            };
            let expires = started
                + Duration::from_millis(
                    tool.definition
                        .policy
                        .timeout_ms
                        .min(self.config.max_timeout_ms),
                );
            self.authorize(&caller, &tool.definition)?;
            if !tool.input.is_valid(&arguments) {
                return Err(Error::new(
                    "INVALID_ARGUMENT",
                    "Input does not match schema",
                ));
            }
            if Instant::now() >= expires {
                return Err(Self::timeout());
            }
            let mut state = self.state.lock().unwrap();
            let session = state
                .session
                .as_ref()
                .filter(|s| s.id == session_id && s.ready)
                .ok_or_else(|| Error::new("NOT_READY", "Execution host changed before dispatch"))?;
            if state.pending.contains_key(&request_id) {
                return Err(Error::new(
                    "DUPLICATE_REQUEST",
                    "Request is already running",
                ));
            }
            if state.pending.len() >= self.config.max_concurrency
                || state
                    .pending
                    .values()
                    .filter(|p| p.tool.definition.name == name)
                    .count()
                    >= tool.definition.policy.max_concurrency
            {
                return Err(Error::new("BUSY", "Too many active calls"));
            }
            let deadline = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
                + expires
                    .saturating_duration_since(Instant::now())
                    .as_millis() as u64;
            let event = BridgeEvent::Call {
                session_id: session_id.clone(),
                request_id: request_id.clone(),
                name: name.into(),
                arguments,
                deadline,
                caller: caller.clone(),
            };
            let sink = session.sink.clone();
            let delivery = session.delivery.clone();
            let (sender, receiver) = oneshot::channel();
            state.pending.insert(
                request_id.clone(),
                Pending {
                    session_id,
                    tool,
                    caller: caller.clone(),
                    started,
                    expires,
                    sink: sink.clone(),
                    delivery: delivery.clone(),
                    sender,
                },
            );
            Ok((sink, delivery, event, receiver, expires))
        })();
        let (sink, delivery, event, mut receiver, expires) = match admitted {
            Ok(admitted) => admitted,
            Err(error) => {
                self.report(&request_id, name, &caller, started, "failed", Some(&error));
                return Err(error);
            }
        };
        let _guard = CallGuard {
            runtime: self.clone(),
            request_id: request_id.clone(),
        };
        {
            let active = delivery.lock().unwrap();
            if *active && self.state.lock().unwrap().pending.contains_key(&request_id) {
                if let BridgeEvent::Call { session_id, .. } = &event {
                    let _ = sink(BridgeEvent::State {
                        session_id: session_id.clone(),
                        pending_count: self.pending_count(),
                        event: Self::event(&request_id, name, &caller, started, "started", None),
                    });
                }
                // A caller may cancel while the started event is being delivered.
                if self.state.lock().unwrap().pending.contains_key(&request_id) {
                    if let Err(error) = sink(event) {
                        // Drop the delivery lock before cancellation acquires it.
                        drop(active);
                        self.cancel(&request_id, error);
                    }
                }
            }
        }
        let result = tokio::select! {
            biased;
            result = &mut receiver => result,
            _ = cancellation.cancelled() => {
                self.cancel(&request_id, Error::unknown("CANCELLED", "Call cancelled"));
                receiver.await
            }
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(expires)) => {
                self.cancel(&request_id, Self::timeout());
                receiver.await
            }
        };
        result.unwrap_or_else(|_| Err(Error::unknown("NOT_READY", "Runtime disconnected")))
    }
}

struct CallGuard {
    runtime: CapabilityRuntime,
    request_id: String,
}
impl Drop for CallGuard {
    fn drop(&mut self) {
        self.runtime.cancel(
            &self.request_id,
            Error::unknown("CANCELLED", "Caller disconnected"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn ipc_invoke_derives_caller_and_uses_host_authorization() {
        let runtime = CapabilityRuntime::new(RuntimeConfig {
            authorizer: Some(Arc::new(|caller, _| {
                assert_eq!(caller, &Caller::local("trusted"));
                Err(Error::new("UNAUTHORIZED", "Host denied access"))
            })),
            ..Default::default()
        })
        .unwrap();
        let tool = serde_json::from_value(json!({
            "name":"test.echo", "description":"Echo", "inputSchema":{"type":"object"}, "outputSchema":{"type":"object"}
        })).unwrap();
        let session = runtime
            .connect(
                "trusted".into(),
                vec![tool],
                Arc::new(|event| {
                    assert!(!matches!(event, BridgeEvent::Call { .. }));
                    Ok(())
                }),
            )
            .unwrap();
        runtime.ready("trusted", &session).unwrap();
        let id = Uuid::new_v4().to_string();
        assert_eq!(
            runtime
                .invoke("other", &session, id.clone(), "test.echo", json!({}))
                .await
                .unwrap_err()
                .code,
            "STALE_SESSION"
        );
        assert_eq!(
            runtime
                .invoke("trusted", &session, id, "test.echo", json!({}))
                .await
                .unwrap_err()
                .code,
            "UNAUTHORIZED"
        );
        assert_eq!(runtime.pending_count(), 0);
    }
}
