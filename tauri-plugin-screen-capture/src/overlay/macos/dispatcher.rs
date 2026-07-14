use std::panic::AssertUnwindSafe;

use tauri::{AppHandle, Runtime};
use tokio::sync::oneshot;

use crate::{models::CaptureErrorCode, Error, Result};

pub type MainThreadTask = Box<dyn FnOnce() + Send + 'static>;

pub trait MainThreadDispatcher: Send + Sync {
    fn dispatch(&self, task: MainThreadTask) -> Result<()>;
}

pub struct TauriMainThreadDispatcher<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> TauriMainThreadDispatcher<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> MainThreadDispatcher for TauriMainThreadDispatcher<R> {
    fn dispatch(&self, task: MainThreadTask) -> Result<()> {
        self.app.run_on_main_thread(task).map_err(|error| {
            Error::new(
                CaptureErrorCode::Internal,
                format!("无法调度 macOS 浮层主线程任务: {error}"),
                true,
            )
        })
    }
}

pub async fn request<T: Send + 'static>(
    dispatcher: &dyn MainThreadDispatcher,
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let (sender, receiver) = oneshot::channel();
    dispatcher.dispatch(Box::new(move || {
        let _ = sender.send(run_catching_objc_exception(operation));
    }))?;
    receiver.await.map_err(|_| {
        Error::new(
            CaptureErrorCode::Internal,
            "macOS 浮层主线程任务未返回结果",
            true,
        )
    })?
}

fn run_catching_objc_exception<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    match objc2::exception::catch(AssertUnwindSafe(operation)) {
        Ok(result) => result,
        Err(exception) => {
            let detail = exception
                .map(|exception| format!("{exception:?}"))
                .unwrap_or_else(|| "nil Objective-C exception".to_string());
            Err(Error::new(
                CaptureErrorCode::Internal,
                format!("macOS 浮层原生调用失败: {detail}"),
                true,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::{NSException, NSGenericException, NSString};

    struct InlineDispatcher;

    impl MainThreadDispatcher for InlineDispatcher {
        fn dispatch(&self, task: MainThreadTask) -> crate::Result<()> {
            task();
            Ok(())
        }
    }

    #[tokio::test]
    async fn request_returns_main_thread_result() {
        let value = request(&InlineDispatcher, || Ok::<_, crate::Error>(41 + 1))
            .await
            .unwrap();

        assert_eq!(value, 42);
    }

    #[test]
    fn objective_c_exception_becomes_overlay_error() {
        let result = run_catching_objc_exception(|| -> crate::Result<()> {
            let reason = NSString::from_str("unsupported NSScreen selector");
            let exception = NSException::new(unsafe { NSGenericException }, Some(&reason), None)
                .expect("create NSException");
            exception.raise();
        });

        let error = result.expect_err("Objective-C exception must become an error");
        assert!(error.to_string().contains("unsupported NSScreen selector"));
    }
}
