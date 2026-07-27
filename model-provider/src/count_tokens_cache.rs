//! In-memory cache for `count_tokens` results.
//!
//! The TypeScript client memoizes count-tokens responses per turn so repeated
//! estimations (status line, autocompact checks, pre-flight sizing) do not each
//! pay a network round-trip. This mirrors that with a small TTL cache keyed by
//! `(model, tool_schema_hash, message_hash)`.
//!
//! Hashing uses `std::hash::DefaultHasher` to avoid pulling in a crypto hash:
//! the key only needs to be collision-resistant enough to distinguish distinct
//! requests within a process, not to be cryptographically secure.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use orbcode_protocol::{ProviderToolDefinition, TranscriptMessage};

use crate::types::ProviderRequest;

/// Default time-to-live for a cached count-tokens result. The TypeScript client
/// recomputes per turn rather than holding a long-lived cache, so a short TTL
/// keeps estimates fresh while still collapsing the burst of calls that happen
/// within a single turn.
pub const DEFAULT_COUNT_TOKENS_TTL: Duration = Duration::from_secs(300);

/// When the cache grows past this many entries, an insert sweeps expired
/// entries. Bounds the process-wide cache against unbounded growth from
/// per-turn keys that are each queried only once.
const SWEEP_THRESHOLD: usize = 256;

/// Cache key derived from the request's model, tool schema, and messages.
///
/// Two requests that would produce the same count-tokens response collapse to
/// the same key; any change to the model id, the tool definitions, or the
/// message history yields a different key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CountTokensCacheKey(u64);

impl CountTokensCacheKey {
    /// Build a key from the model id and the JSON renderings of the tool
    /// schema and messages. Callers pass already-serialized strings so the key
    /// is independent of how the request body is later assembled.
    pub fn from_parts(model: &str, tools_json: &str, messages_json: &str) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        model.hash(&mut hasher);
        // A length-prefixed separator avoids collisions where a boundary shift
        // between the two JSON blobs would otherwise hash identically.
        tools_json.len().hash(&mut hasher);
        tools_json.hash(&mut hasher);
        messages_json.hash(&mut hasher);
        Self(hasher.finish())
    }

    /// Build a key by hashing model, tools, and messages structurally — without
    /// materializing large intermediate JSON strings.
    pub fn from_request(
        model: &str,
        tools: &[ProviderToolDefinition],
        messages: &[TranscriptMessage],
    ) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        model.hash(&mut hasher);

        tools.len().hash(&mut hasher);
        for tool in tools {
            tool.name.hash(&mut hasher);
            tool.description.hash(&mut hasher);
            hash_json_value(&tool.input_schema, &mut hasher);
        }

        let mut writer = HashingWriter(&mut hasher);
        let _ = serde_json::to_writer(&mut writer, messages);

        Self(hasher.finish())
    }

    /// Build a key covering **every** field that
    /// `build_anthropic_count_tokens_request_body` folds into the request.
    ///
    /// [`from_request`](Self::from_request) hashed only model/tools/messages,
    /// but the count-tokens body also incorporates `context` (inserted as
    /// `messages[0]`), `prompt` (the sole message when `messages` is empty),
    /// the `system_prompt`, the `disable_thinking`/`effort` thinking block, and
    /// the `anthropic_betas` / `extra_body` options merged into the body. Two
    /// requests differing only in those fields must not collide on the key, or
    /// the second returns the first's (wrong) count.
    pub fn from_provider_request(request: &ProviderRequest) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        request.model.hash(&mut hasher);

        request.tools.len().hash(&mut hasher);
        for tool in &request.tools {
            tool.name.hash(&mut hasher);
            tool.description.hash(&mut hasher);
            hash_json_value(&tool.input_schema, &mut hasher);
        }

        // Length-prefix the free-form strings so a boundary shift between them
        // cannot hash to the same stream.
        request.prompt.len().hash(&mut hasher);
        request.prompt.hash(&mut hasher);
        request.system_prompt.len().hash(&mut hasher);
        request.system_prompt.hash(&mut hasher);

        request.disable_thinking.hash(&mut hasher);
        request
            .effort
            .map(|effort| effort.as_str())
            .hash(&mut hasher);

        let mut writer = HashingWriter(&mut hasher);
        let _ = serde_json::to_writer(&mut writer, &request.messages);
        // Feed a separator so message/context byte streams cannot fuse.
        0xFFu8.hash(&mut hasher);
        let mut writer = HashingWriter(&mut hasher);
        let _ = serde_json::to_writer(&mut writer, &request.context);

        // `apply_anthropic_betas` / `apply_extra_body` fold these into the
        // count-tokens body too (request/anthropic.rs). `extra_body` in
        // particular can override `system`/`messages`, so two requests differing
        // only here must not collide.
        0xFEu8.hash(&mut hasher);
        request.options.anthropic_betas.len().hash(&mut hasher);
        for beta in &request.options.anthropic_betas {
            beta.hash(&mut hasher);
        }
        0xFDu8.hash(&mut hasher);
        let mut writer = HashingWriter(&mut hasher);
        let _ = serde_json::to_writer(&mut writer, &request.options.extra_body);

        Self(hasher.finish())
    }
}

/// Recursively hash a `serde_json::Value` without serializing it to a string.
fn hash_json_value(value: &serde_json::Value, hasher: &mut impl Hasher) {
    use serde_json::Value;
    match value {
        Value::Null => 0u8.hash(hasher),
        Value::Bool(b) => {
            1u8.hash(hasher);
            b.hash(hasher);
        }
        Value::Number(n) => {
            2u8.hash(hasher);
            // Hash the canonical display form so integer 1 and float 1.0 that
            // display identically produce the same hash.
            n.to_string().hash(hasher);
        }
        Value::String(s) => {
            3u8.hash(hasher);
            s.hash(hasher);
        }
        Value::Array(arr) => {
            4u8.hash(hasher);
            arr.len().hash(hasher);
            for item in arr {
                hash_json_value(item, hasher);
            }
        }
        Value::Object(map) => {
            5u8.hash(hasher);
            map.len().hash(hasher);
            for (key, val) in map {
                key.hash(hasher);
                hash_json_value(val, hasher);
            }
        }
    }
}

/// An `io::Write` adapter that feeds bytes into a `Hasher` instead of
/// allocating a buffer. Used to hash serde-serialized data without
/// materializing the full JSON string.
struct HashingWriter<'a>(&'a mut std::collections::hash_map::DefaultHasher);

impl io::Write for HashingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Entry {
    value: usize,
    stored_at: Instant,
}

/// Thread-safe TTL cache for count-tokens results with hit/miss metrics.
pub struct CountTokensCache {
    ttl: Duration,
    entries: Mutex<HashMap<u64, Entry>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl Default for CountTokensCache {
    fn default() -> Self {
        Self::new()
    }
}

impl CountTokensCache {
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_COUNT_TOKENS_TTL)
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Look up a cached value, recording a hit or miss. Expired entries count as
    /// a miss and are evicted.
    pub fn get(&self, key: CountTokensCacheKey) -> Option<usize> {
        self.get_at(key, Instant::now())
    }

    /// Insert a value, stamping it with the current time.
    pub fn insert(&self, key: CountTokensCacheKey, value: usize) {
        self.insert_at(key, value, Instant::now());
    }

    /// [`get`](Self::get) with an injected clock for deterministic TTL tests.
    pub fn get_at(&self, key: CountTokensCacheKey, now: Instant) -> Option<usize> {
        let mut entries = self.entries.lock().expect("count-tokens cache poisoned");
        if let Some(entry) = entries.get(&key.0) {
            if now.duration_since(entry.stored_at) < self.ttl {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Some(entry.value);
            }
            entries.remove(&key.0);
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// [`insert`](Self::insert) with an injected clock for deterministic TTL
    /// tests.
    pub fn insert_at(&self, key: CountTokensCacheKey, value: usize, now: Instant) {
        let mut entries = self.entries.lock().expect("count-tokens cache poisoned");
        entries.insert(
            key.0,
            Entry {
                value,
                stored_at: now,
            },
        );
        // Each turn's key is unique (messages grow), so expired entries are
        // otherwise only removed lazily on a re-query that never comes. Sweep
        // expired entries once the map grows past a threshold to bound growth.
        if entries.len() > SWEEP_THRESHOLD {
            let ttl = self.ttl;
            entries.retain(|_, entry| now.duration_since(entry.stored_at) < ttl);
        }
    }

    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .expect("count-tokens cache poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_inputs_produce_distinct_keys() {
        let base = CountTokensCacheKey::from_parts("claude", "[tool]", "[msg]");
        assert_ne!(
            base,
            CountTokensCacheKey::from_parts("claude-2", "[tool]", "[msg]")
        );
        assert_ne!(
            base,
            CountTokensCacheKey::from_parts("claude", "[tool2]", "[msg]")
        );
        assert_ne!(
            base,
            CountTokensCacheKey::from_parts("claude", "[tool]", "[msg2]")
        );
        // Identical inputs are stable.
        assert_eq!(
            base,
            CountTokensCacheKey::from_parts("claude", "[tool]", "[msg]")
        );
    }

    #[test]
    fn boundary_shift_does_not_collide() {
        // Without the length prefix these two would hash the same stream.
        let a = CountTokensCacheKey::from_parts("m", "ab", "c");
        let b = CountTokensCacheKey::from_parts("m", "a", "bc");
        assert_ne!(a, b);
    }

    #[test]
    fn hit_and_miss_counters_track_lookups() {
        let cache = CountTokensCache::new();
        let key = CountTokensCacheKey::from_parts("m", "t", "msg");
        assert_eq!(cache.get(key), None);
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);

        cache.insert(key, 1234);
        assert_eq!(cache.get(key), Some(1234));
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 1);
    }

    #[test]
    fn expired_entries_are_evicted_and_count_as_miss() {
        let cache = CountTokensCache::with_ttl(Duration::from_secs(10));
        let key = CountTokensCacheKey::from_parts("m", "t", "msg");
        let t0 = Instant::now();
        cache.insert_at(key, 42, t0);

        // Within TTL: hit.
        assert_eq!(cache.get_at(key, t0 + Duration::from_secs(5)), Some(42));
        assert_eq!(cache.hits(), 1);

        // Past TTL: miss + eviction.
        assert_eq!(cache.get_at(key, t0 + Duration::from_secs(11)), None);
        assert_eq!(cache.misses(), 1);
        assert!(cache.is_empty());
    }

    fn tool(name: &str, schema: serde_json::Value) -> ProviderToolDefinition {
        ProviderToolDefinition {
            name: name.into(),
            description: format!("desc for {name}"),
            input_schema: schema,
        }
    }

    fn msg(content: &str) -> TranscriptMessage {
        TranscriptMessage::new(orbcode_protocol::MessageRole::User, content)
    }

    #[test]
    fn from_request_stable_for_identical_inputs() {
        let tools = vec![tool("read", serde_json::json!({"type": "object"}))];
        let msgs = vec![msg("hello")];
        let a = CountTokensCacheKey::from_request("claude", &tools, &msgs);
        let b = CountTokensCacheKey::from_request("claude", &tools, &msgs);
        assert_eq!(a, b);
    }

    #[test]
    fn from_request_differs_on_model_change() {
        let tools = vec![tool("read", serde_json::json!({}))];
        let msgs = vec![msg("hello")];
        let a = CountTokensCacheKey::from_request("claude", &tools, &msgs);
        let b = CountTokensCacheKey::from_request("claude-2", &tools, &msgs);
        assert_ne!(a, b);
    }

    #[test]
    fn from_request_differs_on_tool_schema_change() {
        let msgs = vec![msg("hello")];
        let a = CountTokensCacheKey::from_request(
            "m",
            &[tool("t", serde_json::json!({"type": "object"}))],
            &msgs,
        );
        let b = CountTokensCacheKey::from_request(
            "m",
            &[tool("t", serde_json::json!({"type": "string"}))],
            &msgs,
        );
        assert_ne!(a, b);
    }

    #[test]
    fn from_request_differs_on_message_change() {
        let tools = vec![tool("t", serde_json::json!({}))];
        let a = CountTokensCacheKey::from_request("m", &tools, &[msg("hello")]);
        let b = CountTokensCacheKey::from_request("m", &tools, &[msg("world")]);
        assert_ne!(a, b);
    }

    fn provider_request() -> ProviderRequest {
        ProviderRequest {
            session_id: "s".into(),
            prompt: "prompt".into(),
            context: orbcode_protocol::TurnContext::default(),
            messages: vec![msg("hello")],
            system_prompt: "system".into(),
            tools: vec![tool("read", serde_json::json!({"type": "object"}))],
            model: "claude".into(),
            base_url: String::new(),
            api_key: None,
            auth_token: None,
            disable_thinking: false,
            effort: None,
            options: crate::types::ProviderRequestOptions::default(),
        }
    }

    #[test]
    fn from_provider_request_differs_on_system_prompt() {
        let a = provider_request();
        let mut b = provider_request();
        b.system_prompt = "different system".into();
        assert_ne!(
            CountTokensCacheKey::from_provider_request(&a),
            CountTokensCacheKey::from_provider_request(&b)
        );
    }

    #[test]
    fn from_provider_request_differs_on_prompt() {
        let a = provider_request();
        let mut b = provider_request();
        b.prompt = "different prompt".into();
        assert_ne!(
            CountTokensCacheKey::from_provider_request(&a),
            CountTokensCacheKey::from_provider_request(&b)
        );
    }

    #[test]
    fn from_provider_request_differs_on_context() {
        let a = provider_request();
        let mut b = provider_request();
        b.context.current_date = "2099-12-31".into();
        assert_ne!(
            CountTokensCacheKey::from_provider_request(&a),
            CountTokensCacheKey::from_provider_request(&b)
        );
    }

    #[test]
    fn from_provider_request_differs_on_thinking_config() {
        let a = provider_request();
        let mut b = provider_request();
        b.disable_thinking = true;
        assert_ne!(
            CountTokensCacheKey::from_provider_request(&a),
            CountTokensCacheKey::from_provider_request(&b)
        );

        let mut c = provider_request();
        c.effort = Some(orbcode_protocol::EffortLevel::High);
        assert_ne!(
            CountTokensCacheKey::from_provider_request(&a),
            CountTokensCacheKey::from_provider_request(&c)
        );
    }

    #[test]
    fn from_provider_request_differs_on_anthropic_betas() {
        let a = provider_request();
        let mut b = provider_request();
        b.options.anthropic_betas = vec!["context-1m-2025-08-07".into()];
        assert_ne!(
            CountTokensCacheKey::from_provider_request(&a),
            CountTokensCacheKey::from_provider_request(&b)
        );
    }

    #[test]
    fn from_provider_request_differs_on_extra_body() {
        // extra_body can override system/messages in the count-tokens body, so
        // it must be part of the key.
        let a = provider_request();
        let mut b = provider_request();
        b.options.extra_body.insert(
            "system".into(),
            serde_json::Value::String("overridden".into()),
        );
        assert_ne!(
            CountTokensCacheKey::from_provider_request(&a),
            CountTokensCacheKey::from_provider_request(&b)
        );
    }

    #[test]
    fn insert_sweeps_expired_entries_past_threshold() {
        let cache = CountTokensCache::with_ttl(Duration::from_secs(10));
        let t0 = Instant::now();
        // Fill past the sweep threshold with entries that will all expire.
        for i in 0..=(SWEEP_THRESHOLD as u64) {
            cache.insert_at(CountTokensCacheKey(i), i as usize, t0);
        }
        assert!(cache.len() > SWEEP_THRESHOLD);

        // A later insert (past TTL) should sweep the now-expired entries.
        let later = t0 + Duration::from_secs(11);
        cache.insert_at(CountTokensCacheKey(u64::MAX), 1, later);
        assert_eq!(
            cache.len(),
            1,
            "expired entries must be swept once the map grows past the threshold"
        );
    }

    #[test]
    fn from_request_differs_on_tool_order() {
        let msgs = vec![msg("hi")];
        let a = CountTokensCacheKey::from_request(
            "m",
            &[
                tool("a", serde_json::json!({})),
                tool("b", serde_json::json!({})),
            ],
            &msgs,
        );
        let b = CountTokensCacheKey::from_request(
            "m",
            &[
                tool("b", serde_json::json!({})),
                tool("a", serde_json::json!({})),
            ],
            &msgs,
        );
        assert_ne!(a, b);
    }
}
