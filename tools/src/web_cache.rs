//! Process-local content cache and domain allow/block policy for the web tools.
//!
//! The cache backs WebFetch with a small LRU keyed by request URL, holding the
//! already-converted text for a fixed TTL so repeated fetches of the same URL
//! within the window return byte-identical content without a network round trip.
//! The domain policy provides allow/block preflight shared by WebFetch (target
//! host) and WebSearch (result hosts).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Instant;

/// Content cache time-to-live: 15 minutes, matching the TypeScript CLI.
pub(crate) const CACHE_TTL_MS: u64 = 15 * 60 * 1000;

/// Maximum number of distinct URLs retained before LRU eviction kicks in.
pub(crate) const MAX_CACHE_ENTRIES: usize = 100;

const ENV_ALLOWED_DOMAINS: &str = "CLAUDE_CODE_WEB_ALLOWED_DOMAINS";
const ENV_BLOCKED_DOMAINS: &str = "CLAUDE_CODE_WEB_BLOCKED_DOMAINS";

// ---------------------------------------------------------------------------
// Logical clock
//
// Time is read through `now_ms()` so tests can advance it without sleeping. The
// base is a real monotonic `Instant`; the test offset is normally zero.
// ---------------------------------------------------------------------------

fn clock_base() -> Instant {
    static BASE: OnceLock<Instant> = OnceLock::new();
    *BASE.get_or_init(Instant::now)
}

static TEST_CLOCK_OFFSET_MS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static TEST_CLOCK_FROZEN_BASE_MS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn now_ms() -> u64 {
    #[cfg(test)]
    {
        let frozen = TEST_CLOCK_FROZEN_BASE_MS.load(Ordering::SeqCst);
        if frozen != 0 {
            return frozen + TEST_CLOCK_OFFSET_MS.load(Ordering::SeqCst);
        }
    }
    clock_base().elapsed().as_millis() as u64 + TEST_CLOCK_OFFSET_MS.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Content cache
// ---------------------------------------------------------------------------

/// The fetched-and-processed payload retained for a single URL. Stores the
/// post-conversion, post-truncation content (without any user-prompt wrapper)
/// plus the response facts needed to rebuild an identical tool result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CachedContent {
    pub content: String,
    pub final_url: String,
    pub status_code: u16,
    pub content_type: String,
    pub converted_to_markdown: bool,
    pub redirected: bool,
    pub redirect_count: u32,
    pub truncated: bool,
    pub response_bytes: usize,
}

struct CacheEntry {
    content: CachedContent,
    inserted_at_ms: u64,
    last_access_seq: u64,
}

#[derive(Default)]
struct CacheState {
    entries: HashMap<String, CacheEntry>,
    access_counter: u64,
}

fn cache() -> &'static Mutex<CacheState> {
    static CACHE: OnceLock<Mutex<CacheState>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(CacheState::default()))
}

fn lock_cache() -> std::sync::MutexGuard<'static, CacheState> {
    cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A cache hit: the stored content plus how long ago it was inserted.
pub(crate) struct CacheHit {
    pub content: CachedContent,
    pub age_ms: u64,
}

/// Cache key that binds the URL to the active domain policy. A cache entry
/// populated under one security context (e.g. an allow-listed internal domain)
/// must NOT be served to a context with a different policy — otherwise a stale
/// hit would bypass the private-address / allow-list check that the fresh path
/// (re-resolution + policy evaluation) would apply. Keying by the policy
/// signature scopes hits to the exact policy that validated them; any change
/// (removed allow-list entry, different `settings.env`) is a miss that re-runs
/// the full SSRF validation.
fn scoped_key(url: &str, context: Option<&crate::ToolContext>) -> String {
    format!("{:016x}\u{1f}{url}", effective_policy(context).signature())
}

/// Look up `url` within the active policy context. Returns the entry only when
/// present and still within TTL; expired entries are removed eagerly. A hit
/// refreshes the LRU recency.
pub(crate) fn lookup(url: &str, context: Option<&crate::ToolContext>) -> Option<CacheHit> {
    let key = scoped_key(url, context);
    let now = now_ms();
    let mut state = lock_cache();
    let expired = {
        let entry = state.entries.get(&key)?;
        now.saturating_sub(entry.inserted_at_ms) >= CACHE_TTL_MS
    };
    if expired {
        state.entries.remove(&key);
        return None;
    }
    state.access_counter += 1;
    let seq = state.access_counter;
    let entry = state.entries.get_mut(&key)?;
    entry.last_access_seq = seq;
    Some(CacheHit {
        content: entry.content.clone(),
        age_ms: now.saturating_sub(entry.inserted_at_ms),
    })
}

/// Insert (or replace) the cached content for `url` under the active policy
/// context, evicting the least-recently-used entry first when at capacity.
pub(crate) fn store(url: &str, content: CachedContent, context: Option<&crate::ToolContext>) {
    let url = &scoped_key(url, context);
    let now = now_ms();
    let mut state = lock_cache();
    state.access_counter += 1;
    let seq = state.access_counter;

    if !state.entries.contains_key(url)
        && state.entries.len() >= MAX_CACHE_ENTRIES
        && let Some(victim) = state
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_access_seq)
            .map(|(key, _)| key.clone())
    {
        state.entries.remove(&victim);
    }

    state.entries.insert(
        url.to_string(),
        CacheEntry {
            content,
            inserted_at_ms: now,
            last_access_seq: seq,
        },
    );
}

// ---------------------------------------------------------------------------
// Domain policy
// ---------------------------------------------------------------------------

/// Allow/block lists of bare domains (e.g. `example.com`). An entry matches a
/// host when it equals the host or is a parent domain of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DomainPolicy {
    pub allowlist: Vec<String>,
    pub blocklist: Vec<String>,
}

impl DomainPolicy {
    fn is_empty(&self) -> bool {
        self.allowlist.is_empty() && self.blocklist.is_empty()
    }

    /// Order-independent fingerprint of the allow/block lists, used to scope
    /// cache entries to the policy that validated them.
    fn signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut allow = self.allowlist.clone();
        let mut block = self.blocklist.clone();
        allow.sort();
        block.sort();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        allow.hash(&mut hasher);
        // Separate the two lists so `[a]`/`[]` and `[]`/`[a]` do not collide.
        0xffff_u64.hash(&mut hasher);
        block.hash(&mut hasher);
        hasher.finish()
    }
}

/// Why a host failed preflight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DomainRejection {
    /// Host matched an explicit blocklist entry.
    Blocked(String),
    /// An allowlist is configured and the host is not on it.
    NotAllowed(String),
}

impl DomainRejection {
    pub(crate) fn reason(&self) -> &str {
        match self {
            DomainRejection::Blocked(_) => "blocklist",
            DomainRejection::NotAllowed(_) => "not_in_allowlist",
        }
    }

    pub(crate) fn message(&self, url: &str) -> String {
        match self {
            DomainRejection::Blocked(host) => format!(
                "Domain `{host}` is blocked by the configured web domain blocklist; refusing to fetch {url}."
            ),
            DomainRejection::NotAllowed(host) => format!(
                "Domain `{host}` is not in the configured web domain allowlist; refusing to fetch {url}."
            ),
        }
    }
}

fn programmatic_policy() -> &'static RwLock<DomainPolicy> {
    static POLICY: OnceLock<RwLock<DomainPolicy>> = OnceLock::new();
    POLICY.get_or_init(|| RwLock::new(DomainPolicy::default()))
}

fn normalize_domains(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.trim_start_matches("www.").to_ascii_lowercase())
        .collect()
}

fn env_domains(key: &str, context: Option<&crate::ToolContext>) -> Vec<String> {
    let value = match context {
        Some(ctx) => ctx.resolve_env(key),
        None => orbcode_config::resolve_process_env(key),
    };
    value
        .map(|value| normalize_domains(&value))
        .unwrap_or_default()
}

/// Replace the programmatic domain policy. Lists are normalized (trimmed,
/// lowercased, `www.` stripped); empty entries are dropped.
///
/// Production configuration arrives through the environment overrides read by
/// [`effective_policy`]; this setter drives the policy directly from tests.
#[cfg(test)]
pub(crate) fn set_domain_policy(allowlist: &[String], blocklist: &[String]) {
    let policy = DomainPolicy {
        allowlist: allowlist
            .iter()
            .flat_map(|item| normalize_domains(item))
            .collect(),
        blocklist: blocklist
            .iter()
            .flat_map(|item| normalize_domains(item))
            .collect(),
    };
    *programmatic_policy()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = policy;
}

/// The active policy: the programmatic policy unioned with the environment
/// overrides (`CLAUDE_CODE_WEB_ALLOWED_DOMAINS` / `..._BLOCKED_DOMAINS`).
pub(crate) fn effective_policy(context: Option<&crate::ToolContext>) -> DomainPolicy {
    let mut policy = programmatic_policy()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    policy
        .allowlist
        .extend(env_domains(ENV_ALLOWED_DOMAINS, context));
    policy
        .blocklist
        .extend(env_domains(ENV_BLOCKED_DOMAINS, context));
    policy
}

pub(crate) fn host_matches(host: &str, domain: &str) -> bool {
    let host = host.trim_start_matches("www.").to_ascii_lowercase();
    let domain = domain.trim_start_matches("www.").to_ascii_lowercase();
    host == domain || host.ends_with(&format!(".{domain}"))
}

fn evaluate(policy: &DomainPolicy, host: &str) -> Option<DomainRejection> {
    if policy
        .blocklist
        .iter()
        .any(|domain| host_matches(host, domain))
    {
        return Some(DomainRejection::Blocked(host.to_string()));
    }
    if !policy.allowlist.is_empty()
        && !policy
            .allowlist
            .iter()
            .any(|domain| host_matches(host, domain))
    {
        return Some(DomainRejection::NotAllowed(host.to_string()));
    }
    None
}

/// Preflight a host against the active policy. `Ok(())` means the host may be
/// contacted; `Err` carries the reason it was rejected.
pub(crate) fn check_domain(
    host: &str,
    context: Option<&crate::ToolContext>,
) -> Result<(), DomainRejection> {
    let policy = effective_policy(context);
    if policy.is_empty() {
        return Ok(());
    }
    match evaluate(&policy, host) {
        Some(rejection) => Err(rejection),
        None => Ok(()),
    }
}

/// Whether a result host is permitted under the active policy. Used by
/// WebSearch to drop disallowed result domains.
pub(crate) fn host_allowed(host: &str, context: Option<&crate::ToolContext>) -> bool {
    let policy = effective_policy(context);
    policy.is_empty() || evaluate(&policy, host).is_none()
}

/// Whether `host` is explicitly named on the active allowlist. Unlike
/// [`host_allowed`], an empty policy returns `false` here — the SSRF guard uses
/// this to require a deliberate allowlist entry before contacting an internal
/// address, rather than treating "no policy" as blanket permission.
pub(crate) fn host_explicitly_allowlisted(
    host: &str,
    context: Option<&crate::ToolContext>,
) -> bool {
    let policy = effective_policy(context);
    policy
        .allowlist
        .iter()
        .any(|domain| host_matches(host, domain))
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Serializes every test that touches the process-global cache / domain policy /
/// network counter. Shared by this module's unit tests and the crate integration
/// tests so the two suites cannot clobber each other's global state.
#[cfg(test)]
pub(crate) static TEST_GUARD: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) fn advance_clock_for_tests(millis: u64) {
    TEST_CLOCK_OFFSET_MS.fetch_add(millis, Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    TEST_CLOCK_FROZEN_BASE_MS.store(
        clock_base().elapsed().as_millis() as u64 + 1,
        Ordering::SeqCst,
    );
    TEST_CLOCK_OFFSET_MS.store(0, Ordering::SeqCst);
    lock_cache().entries.clear();
    set_domain_policy(&[], &[]);
}

#[cfg(test)]
pub(crate) fn cache_len_for_tests() -> usize {
    lock_cache().entries.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    fn guard() -> MutexGuard<'static, ()> {
        let g = TEST_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reset_for_tests();
        g
    }

    fn sample(tag: &str) -> CachedContent {
        CachedContent {
            content: format!("content-{tag}"),
            final_url: format!("https://example.com/{tag}"),
            status_code: 200,
            content_type: "text/html".into(),
            converted_to_markdown: true,
            redirected: false,
            redirect_count: 0,
            truncated: false,
            response_bytes: 10,
        }
    }

    #[test]
    fn cache_returns_stored_content_within_ttl() {
        let _g = guard();
        store("https://example.com/a", sample("a"), None);
        let hit = lookup("https://example.com/a", None).expect("entry present");
        assert_eq!(hit.content.content, "content-a");
        assert!(hit.age_ms < CACHE_TTL_MS);
    }

    #[test]
    fn cache_expires_after_ttl() {
        let _g = guard();
        store("https://example.com/a", sample("a"), None);
        advance_clock_for_tests(CACHE_TTL_MS / 2);
        assert!(
            lookup("https://example.com/a", None).is_some(),
            "still valid within TTL"
        );
        advance_clock_for_tests(CACHE_TTL_MS);
        assert!(
            lookup("https://example.com/a", None).is_none(),
            "expired at TTL"
        );
        assert_eq!(cache_len_for_tests(), 0, "expired entry removed on lookup");
    }

    #[test]
    fn cache_age_reflects_elapsed_time() {
        let _g = guard();
        store("https://example.com/a", sample("a"), None);
        advance_clock_for_tests(1234);
        let hit = lookup("https://example.com/a", None).expect("entry present");
        assert!(
            hit.age_ms >= 1234 && hit.age_ms < CACHE_TTL_MS,
            "cache age should include the test offset without expiring, got {}",
            hit.age_ms
        );
    }

    #[test]
    fn cache_hit_is_scoped_to_the_domain_policy() {
        let _g = guard();
        // Cached while an allow-list permits the (internal) domain.
        set_domain_policy(&["internal.example".into()], &[]);
        store("https://internal.example/x", sample("secret"), None);
        assert!(
            lookup("https://internal.example/x", None).is_some(),
            "a hit is served under the same policy that cached it"
        );
        // Removing the allow-list changes the policy: the same URL must MISS so
        // the fetch path re-runs SSRF validation instead of serving the stale
        // entry.
        set_domain_policy(&[], &[]);
        assert!(
            lookup("https://internal.example/x", None).is_none(),
            "a changed domain policy must not serve the previously-cached entry"
        );
    }

    #[test]
    fn cache_evicts_least_recently_used_at_capacity() {
        let _g = guard();
        for i in 0..MAX_CACHE_ENTRIES {
            store(
                &format!("https://example.com/{i}"),
                sample(&i.to_string()),
                None,
            );
        }
        // Touch entry 0 so it is the most-recently-used; entry 1 becomes LRU.
        assert!(lookup("https://example.com/0", None).is_some());
        store("https://example.com/new", sample("new"), None);

        assert_eq!(cache_len_for_tests(), MAX_CACHE_ENTRIES);
        assert!(
            lookup("https://example.com/0", None).is_some(),
            "recently used retained"
        );
        assert!(
            lookup("https://example.com/new", None).is_some(),
            "newest retained"
        );
        assert!(
            lookup("https://example.com/1", None).is_none(),
            "LRU evicted"
        );
    }

    #[test]
    fn host_matches_uses_host_not_substring() {
        // Exact host and subdomains match.
        assert!(host_matches("github.com", "github.com"));
        assert!(host_matches("api.github.com", "github.com"));
        assert!(host_matches("www.github.com", "github.com"));
        // A host that merely contains the domain as a substring must NOT match:
        // this is the per-call allow/block-list bypass the review flagged.
        assert!(!host_matches("attacker.com", "github.com"));
        assert!(!host_matches("github.com.evil.com", "github.com"));
        assert!(!host_matches("notgithub.com", "github.com"));
    }

    #[test]
    fn blocklist_rejects_matching_host_and_subdomains() {
        let _g = guard();
        set_domain_policy(&[], &["blocked.com".into()]);
        assert!(matches!(
            check_domain("blocked.com", None),
            Err(DomainRejection::Blocked(_))
        ));
        assert!(matches!(
            check_domain("api.blocked.com", None),
            Err(DomainRejection::Blocked(_))
        ));
        assert!(check_domain("example.com", None).is_ok());
    }

    #[test]
    fn allowlist_restricts_to_listed_domains() {
        let _g = guard();
        set_domain_policy(&["allowed.com".into()], &[]);
        assert!(check_domain("allowed.com", None).is_ok());
        assert!(check_domain("docs.allowed.com", None).is_ok());
        assert!(matches!(
            check_domain("other.com", None),
            Err(DomainRejection::NotAllowed(_))
        ));
    }

    #[test]
    fn blocklist_takes_precedence_over_allowlist() {
        let _g = guard();
        set_domain_policy(&["example.com".into()], &["example.com".into()]);
        assert!(matches!(
            check_domain("example.com", None),
            Err(DomainRejection::Blocked(_))
        ));
    }

    #[test]
    fn empty_policy_allows_everything() {
        let _g = guard();
        assert!(check_domain("anything.com", None).is_ok());
        assert!(host_allowed("anything.com", None));
    }

    #[test]
    fn www_prefix_is_normalized_on_both_sides() {
        let _g = guard();
        set_domain_policy(&[], &["www.blocked.com".into()]);
        assert!(matches!(
            check_domain("www.blocked.com", None),
            Err(DomainRejection::Blocked(_))
        ));
        assert!(matches!(
            check_domain("blocked.com", None),
            Err(DomainRejection::Blocked(_))
        ));
    }

    #[test]
    fn host_allowed_mirrors_check_domain() {
        let _g = guard();
        set_domain_policy(&["allowed.com".into()], &[]);
        assert!(host_allowed("allowed.com", None));
        assert!(!host_allowed("blocked.org", None));
    }
}
