use std::collections::HashMap;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

/// Process-wide registry mapping background task IDs to in-memory cancellation
/// flags owned by the worker that produced the task. The registry is the
/// bridge between TaskStop in the model-facing tools layer and the
/// long-running worker (e.g. a background Agent loop) running elsewhere in
/// the process. Workers register a flag when they start, set/check it
/// themselves, and unregister when they finish.
fn registry() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_background_task_cancel_flag(task_id: &str, flag: Arc<AtomicBool>) {
    registry()
        .lock()
        .expect("background cancel registry mutex")
        .insert(task_id.to_string(), flag);
}

pub fn unregister_background_task_cancel_flag(task_id: &str) {
    registry()
        .lock()
        .expect("background cancel registry mutex")
        .remove(task_id);
}

pub fn has_background_task_cancel_flag(task_id: &str) -> bool {
    registry()
        .lock()
        .expect("background cancel registry mutex")
        .contains_key(task_id)
}

/// Set the registered flag for `task_id` to true and return whether a flag was
/// found. Callers should treat `false` as "no in-process worker is listening"
/// — they may still want to mark the durable record cancelled on disk.
pub fn cancel_background_task(task_id: &str) -> bool {
    let Some(flag) = registry()
        .lock()
        .expect("background cancel registry mutex")
        .get(task_id)
        .cloned()
    else {
        return false;
    };
    flag.store(true, Ordering::SeqCst);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique(label: &str) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        format!("{label}-{nanos}-{}", std::process::id())
    }

    #[test]
    fn cancel_returns_false_when_no_flag_registered() {
        let task_id = unique("missing");
        assert!(!cancel_background_task(&task_id));
    }

    #[test]
    fn cancel_sets_registered_flag_and_returns_true() {
        let task_id = unique("present");
        let flag = Arc::new(AtomicBool::new(false));
        register_background_task_cancel_flag(&task_id, flag.clone());
        assert!(has_background_task_cancel_flag(&task_id));
        assert!(cancel_background_task(&task_id));
        assert!(flag.load(Ordering::SeqCst));
        unregister_background_task_cancel_flag(&task_id);
        assert!(!has_background_task_cancel_flag(&task_id));
    }
}
