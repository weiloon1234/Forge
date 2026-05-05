use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
pub(crate) struct ManagedBackgroundTasks {
    shutting_down: AtomicBool,
    tasks: Mutex<Vec<ManagedBackgroundTask>>,
}

struct ManagedBackgroundTask {
    name: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    completed: tokio::sync::oneshot::Receiver<()>,
    abort: tokio::task::AbortHandle,
}

impl ManagedBackgroundTasks {
    pub(crate) fn register(
        &self,
        name: impl Into<String>,
        shutdown: tokio::sync::oneshot::Sender<()>,
        completed: tokio::sync::oneshot::Receiver<()>,
        abort: tokio::task::AbortHandle,
    ) {
        let task = ManagedBackgroundTask {
            name: name.into(),
            shutdown: Some(shutdown),
            completed,
            abort,
        };

        let mut tasks = self
            .tasks
            .lock()
            .expect("managed background task registry lock poisoned");
        if self.shutting_down.load(Ordering::SeqCst) {
            tracing::warn!(
                task = %task.name,
                "managed background task registered during shutdown; aborting"
            );
            task.abort.abort();
            return;
        }

        tasks.push(task);
    }

    pub(crate) async fn shutdown(&self, timeout: Duration) {
        self.shutting_down.store(true, Ordering::SeqCst);
        let mut tasks = {
            let mut tasks = self
                .tasks
                .lock()
                .expect("managed background task registry lock poisoned");
            std::mem::take(&mut *tasks)
        };

        if tasks.is_empty() {
            return;
        }

        for task in &mut tasks {
            if let Some(shutdown) = task.shutdown.take() {
                let _ = shutdown.send(());
            }
        }

        if timeout.is_zero() {
            tracing::warn!(
                active = tasks.len(),
                "background shutdown timeout disabled; aborting managed background tasks"
            );
            abort_background_tasks(tasks);
            return;
        }

        let task_count = tasks.len();
        tracing::info!(
            active = task_count,
            timeout_ms = timeout.as_millis(),
            "waiting for managed background tasks during shutdown"
        );

        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);

        loop {
            reap_completed_background_tasks(&mut tasks);
            if tasks.is_empty() {
                tracing::info!(active = task_count, "managed background tasks drained");
                return;
            }

            tokio::select! {
                biased;
                _ = &mut deadline => {
                    tracing::warn!(
                        active = tasks.len(),
                        timeout_ms = timeout.as_millis(),
                        "background shutdown timeout elapsed; aborting managed background tasks"
                    );
                    abort_background_tasks(tasks);
                    return;
                }
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
    }
}

fn reap_completed_background_tasks(tasks: &mut Vec<ManagedBackgroundTask>) {
    let mut index = 0;
    while index < tasks.len() {
        match tasks[index].completed.try_recv() {
            Ok(()) | Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                tasks.swap_remove(index);
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                index += 1;
            }
        }
    }
}

fn abort_background_tasks(tasks: Vec<ManagedBackgroundTask>) {
    for task in tasks {
        tracing::warn!(task = %task.name, "aborting managed background task");
        task.abort.abort();
    }
}
