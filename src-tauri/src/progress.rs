use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    instance_id: String,
    stage: String,
    message: String,
    current: usize,
    total: usize,
}

/// Sends `launch-progress` events to the UI while an instance is being prepared.
#[derive(Clone)]
pub struct Progress {
    app: AppHandle,
    instance_id: String,
    current: Arc<Mutex<(String, String)>>,
}

impl Progress {
    pub fn new(app: AppHandle, instance_id: &str) -> Self {
        Self {
            app,
            instance_id: instance_id.to_string(),
            current: Arc::new(Mutex::new((String::new(), String::new()))),
        }
    }

    pub fn stage(&self, stage: &str, message: impl Into<String>) {
        let message = message.into();
        *self.current.lock().unwrap() = (stage.to_string(), message.clone());
        self.emit(stage.to_string(), message, 0, 0);
    }

    pub fn update(&self, current: usize, total: usize) {
        let (stage, message) = self.current.lock().unwrap().clone();
        self.emit(stage, message, current, total);
    }

    fn emit(&self, stage: String, message: String, current: usize, total: usize) {
        let _ = self.app.emit(
            "launch-progress",
            ProgressEvent {
                instance_id: self.instance_id.clone(),
                stage,
                message,
                current,
                total,
            },
        );
    }
}
