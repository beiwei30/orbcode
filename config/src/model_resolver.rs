use orbcode_protocol::ProviderId;

const DEFAULT_ANTHROPIC_OPUS_MODEL: &str = "claude-opus-4-7";
const DEFAULT_ANTHROPIC_SONNET_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_ANTHROPIC_HAIKU_MODEL: &str = "claude-haiku-4-5-20251001";
const DEFAULT_OPENAI_OPUS_MODEL: &str = "o3";
const DEFAULT_OPENAI_SONNET_MODEL: &str = "gpt-4o";
const DEFAULT_OPENAI_HAIKU_MODEL: &str = "gpt-4o-mini";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFamily {
    Opus,
    Sonnet,
    Haiku,
}

impl ModelFamily {
    pub const ALL: [Self; 3] = [Self::Sonnet, Self::Opus, Self::Haiku];

    pub fn alias(self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::Sonnet => "sonnet",
            Self::Haiku => "haiku",
        }
    }

    pub fn env_name(self) -> &'static str {
        match self {
            Self::Opus => "OPUS",
            Self::Sonnet => "SONNET",
            Self::Haiku => "HAIKU",
        }
    }

    pub fn marketing_label(self) -> &'static str {
        match self {
            Self::Opus => "Opus 4.7",
            Self::Sonnet => "Sonnet",
            Self::Haiku => "Haiku",
        }
    }

    pub fn default_description(self) -> &'static str {
        match self {
            Self::Opus => "Opus 4.7 - most capable for complex work",
            Self::Sonnet => "Sonnet 4.6 - best for everyday tasks",
            Self::Haiku => "Haiku 4.5 - fastest for quick answers",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelCapability {
    Effort,
    MaxEffort,
    XHighEffort,
    Thinking,
    AdaptiveThinking,
    InterleavedThinking,
}

impl ModelCapability {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "effort" => Some(Self::Effort),
            "max_effort" => Some(Self::MaxEffort),
            "xhigh_effort" => Some(Self::XHighEffort),
            "thinking" => Some(Self::Thinking),
            "adaptive_thinking" => Some(Self::AdaptiveThinking),
            "interleaved_thinking" => Some(Self::InterleavedThinking),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Effort => "effort",
            Self::MaxEffort => "max_effort",
            Self::XHighEffort => "xhigh_effort",
            Self::Thinking => "thinking",
            Self::AdaptiveThinking => "adaptive_thinking",
            Self::InterleavedThinking => "interleaved_thinking",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderModelResolution {
    pub provider: ProviderId,
    pub requested_setting: Option<String>,
    pub family: Option<ModelFamily>,
    pub model: String,
    pub request_model: String,
    pub display_label: String,
    pub display_name: String,
    pub capabilities: Vec<ModelCapability>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelCapabilities {
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub max_output_tokens_upper_limit: u32,
    pub supports_thinking: bool,
    pub supports_vision: bool,
    pub supports_streaming: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelFamilyOption {
    pub family: ModelFamily,
    pub value: String,
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownModelOption {
    pub label: String,
    pub description: String,
}

pub fn resolve_provider_model<F>(
    provider: ProviderId,
    requested_setting: Option<&str>,
    mut env_lookup: F,
) -> ProviderModelResolution
where
    F: FnMut(&str) -> Option<String>,
{
    let parsed = requested_setting.map(parse_model_setting);
    let family = parsed
        .as_ref()
        .and_then(|parsed| parsed.family)
        .or_else(|| requested_setting.is_none().then_some(ModelFamily::Sonnet));
    let resolved_model = match parsed {
        Some(parsed) if parsed.family.is_some() => {
            let family = parsed.family.expect("family checked above");
            let mut model = resolve_family_model(provider, family, &mut env_lookup);
            model.push_str(parsed.suffix);
            model
        }
        Some(parsed) => parsed.setting.to_string(),
        None => resolve_family_model(provider, ModelFamily::Sonnet, &mut env_lookup),
    };
    let request_model = match provider {
        ProviderId::OpenAi => resolve_openai_model(&resolved_model, &mut env_lookup),
        _ => resolved_model.clone(),
    };
    let request_model = normalize_model_string_for_api(&request_model);
    let display_label = family.map_or_else(
        || request_model.clone(),
        |family| resolve_family_display_label(provider, family, &request_model, &mut env_lookup),
    );
    let capabilities = family
        .and_then(|family| resolve_family_capabilities(provider, family, &mut env_lookup))
        .unwrap_or_else(|| default_capabilities(provider, &request_model));

    ProviderModelResolution {
        provider,
        requested_setting: requested_setting.map(str::to_string),
        family,
        model: resolved_model,
        request_model,
        display_name: format_provider_model_display_name(&display_label, provider),
        display_label,
        capabilities,
    }
}

pub fn resolve_small_fast_model<F>(
    provider: ProviderId,
    mut env_lookup: F,
) -> ProviderModelResolution
where
    F: FnMut(&str) -> Option<String>,
{
    let model = match provider {
        ProviderId::OpenAi => env_lookup("OPENAI_SMALL_FAST_MODEL"),
        _ => None,
    }
    .or_else(|| env_lookup("ANTHROPIC_SMALL_FAST_MODEL"))
    .unwrap_or_else(|| resolve_family_model(provider, ModelFamily::Haiku, &mut env_lookup));
    let request_model = normalize_model_string_for_api(&model);
    let display_label = if request_model == model {
        resolve_family_display_label(
            provider,
            ModelFamily::Haiku,
            &request_model,
            &mut env_lookup,
        )
    } else {
        request_model.clone()
    };
    let capabilities = resolve_family_capabilities(provider, ModelFamily::Haiku, &mut env_lookup)
        .unwrap_or_else(|| default_capabilities(provider, &request_model));

    ProviderModelResolution {
        provider,
        requested_setting: Some("small-fast".to_string()),
        family: Some(ModelFamily::Haiku),
        model,
        request_model,
        display_name: format_provider_model_display_name(&display_label, provider),
        display_label,
        capabilities,
    }
}

pub fn family_model_options<F>(provider: ProviderId, mut env_lookup: F) -> Vec<ModelFamilyOption>
where
    F: FnMut(&str) -> Option<String>,
{
    ModelFamily::ALL
        .into_iter()
        .map(|family| {
            let model = resolve_family_model(provider, family, &mut env_lookup);
            let request_model = match provider {
                ProviderId::OpenAi => resolve_openai_model(&model, &mut env_lookup),
                _ => model.clone(),
            };
            let request_model = normalize_model_string_for_api(&request_model);
            ModelFamilyOption {
                family,
                value: family.alias().to_string(),
                label: resolve_family_display_label(
                    provider,
                    family,
                    &request_model,
                    &mut env_lookup,
                ),
                description: resolve_family_description(provider, family, &mut env_lookup)
                    .or_else(|| {
                        family_model_is_configured(provider, family, &mut env_lookup)
                            .then(|| format!("Custom {} model", capitalize_family(family)))
                    })
                    .unwrap_or_else(|| family.default_description().to_string()),
            }
        })
        .collect()
}

pub fn known_model_option<F>(
    provider: ProviderId,
    model: &str,
    mut env_lookup: F,
) -> Option<KnownModelOption>
where
    F: FnMut(&str) -> Option<String>,
{
    let label = marketing_name_for_model(model)?;
    let Some(family) = known_model_family(model) else {
        return Some(KnownModelOption {
            label,
            description: model.to_string(),
        });
    };
    let current_model = resolve_family_model(provider, family, &mut env_lookup);
    let current_label = marketing_name_for_model(&current_model)?;
    if label != current_label {
        return Some(KnownModelOption {
            label,
            description: format!(
                "Newer version available - select {} for {}",
                capitalize_family(family),
                current_label
            ),
        });
    }

    Some(KnownModelOption {
        label,
        description: model.to_string(),
    })
}

pub fn normalize_model_string_for_api(model: &str) -> String {
    let trimmed = model.trim();
    for suffix in ["[1m]", "[2m]", "[1M]", "[2M]"] {
        if let Some(stripped) = trimmed.strip_suffix(suffix) {
            return stripped.trim().to_string();
        }
    }
    trimmed.to_string()
}

pub fn format_provider_model_display_name(model: &str, _provider: ProviderId) -> String {
    model.to_string()
}

pub fn resolve_openai_model<F>(anthropic_model: &str, mut env_lookup: F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    if let Some(model) = env_lookup("OPENAI_MODEL") {
        return model;
    }

    let clean_model = normalize_model_string_for_api(anthropic_model);
    if let Some(family) = model_family_for_string(&clean_model) {
        let openai_key = format!("OPENAI_DEFAULT_{}_MODEL", family.env_name());
        if let Some(model) = env_lookup(&openai_key) {
            return model;
        }
        let anthropic_key = format!("ANTHROPIC_DEFAULT_{}_MODEL", family.env_name());
        if let Some(model) = env_lookup(&anthropic_key) {
            return model;
        }
        return builtin_openai_family_model(family).to_string();
    }

    clean_model
}

pub fn model_capabilities(model: &str, provider: ProviderId) -> ModelCapabilities {
    let canonical = canonical_model_name(&model.to_ascii_lowercase());
    let (context_window, max_output, max_output_upper, supports_thinking) = match provider {
        ProviderId::OpenAi => openai_model_capabilities(&canonical),
        _ => anthropic_model_capabilities(&canonical),
    };
    ModelCapabilities {
        context_window,
        max_output_tokens: max_output,
        max_output_tokens_upper_limit: max_output_upper,
        supports_thinking,
        supports_vision: supports_vision_for_model(&canonical, provider),
        supports_streaming: true,
    }
}

const DEFAULT_CONTEXT_WINDOW: u32 = 200_000;
const CONTEXT_1M: u32 = 1_000_000;

fn anthropic_model_capabilities(canonical: &str) -> (u32, u32, u32, bool) {
    if canonical.contains("opus-4-7") || canonical.contains("opus-4-6") {
        (CONTEXT_1M, 64_000, 128_000, true)
    } else if canonical.contains("sonnet-4-6") {
        (CONTEXT_1M, 32_000, 128_000, true)
    } else if canonical.contains("opus-4-1") || canonical.ends_with("opus-4") {
        (DEFAULT_CONTEXT_WINDOW, 32_000, 32_000, true)
    } else if canonical.contains("3-5-sonnet") || canonical.contains("3-5-haiku") {
        (DEFAULT_CONTEXT_WINDOW, 8_192, 8_192, false)
    } else if canonical.contains("claude-3-opus") || canonical.contains("claude-3-haiku") {
        (DEFAULT_CONTEXT_WINDOW, 4_096, 4_096, false)
    } else if canonical.contains("claude") {
        (DEFAULT_CONTEXT_WINDOW, 32_000, 64_000, true)
    } else {
        (DEFAULT_CONTEXT_WINDOW, 32_000, 64_000, false)
    }
}

fn openai_model_capabilities(canonical: &str) -> (u32, u32, u32, bool) {
    if canonical.contains("o3") || canonical.contains("o4") {
        (200_000, 100_000, 100_000, false)
    } else if canonical.contains("gpt-4")
        && !canonical.contains("gpt-4o")
        && !canonical.contains("gpt-4-turbo")
        && !canonical.contains("gpt-4-1")
    {
        (8_192, 8_192, 8_192, false)
    } else if canonical.contains("gpt-3.5") || canonical.contains("gpt-35") {
        (16_385, 4_096, 4_096, false)
    } else {
        (128_000, 16_384, 16_384, false)
    }
}

fn supports_vision_for_model(canonical: &str, provider: ProviderId) -> bool {
    match provider {
        ProviderId::Gemini => canonical.contains("claude") || canonical.contains("gemini"),
        ProviderId::Grok => canonical.contains("claude") || canonical.contains("grok"),
        ProviderId::OpenAi => {
            canonical.contains("gpt-4o")
                || canonical.contains("gpt-4-turbo")
                || canonical.contains("gpt-4-1")
                || canonical.contains("o3")
                || canonical.contains("o4")
        }
        _ => canonical.contains("claude"),
    }
}

fn resolve_family_model<F>(provider: ProviderId, family: ModelFamily, env_lookup: &mut F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    provider_family_key(provider, family, "MODEL")
        .and_then(|key| env_lookup(&key))
        .or_else(|| env_lookup(&anthropic_family_key(family, "MODEL")))
        .unwrap_or_else(|| match provider {
            ProviderId::OpenAi => builtin_openai_family_model(family).to_string(),
            _ => builtin_anthropic_family_model(family).to_string(),
        })
}

fn resolve_family_display_label<F>(
    provider: ProviderId,
    family: ModelFamily,
    request_model: &str,
    env_lookup: &mut F,
) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    provider_family_key(provider, family, "MODEL_NAME")
        .and_then(|key| env_lookup(&key))
        .or_else(|| env_lookup(&anthropic_family_key(family, "MODEL_NAME")))
        .unwrap_or_else(|| {
            let builtin = match provider {
                ProviderId::OpenAi => builtin_openai_family_model(family),
                _ => builtin_anthropic_family_model(family),
            };
            if request_model == builtin {
                family.marketing_label().to_string()
            } else {
                request_model.to_string()
            }
        })
}

fn resolve_family_description<F>(
    provider: ProviderId,
    family: ModelFamily,
    env_lookup: &mut F,
) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    provider_family_key(provider, family, "MODEL_DESCRIPTION")
        .and_then(|key| env_lookup(&key))
        .or_else(|| env_lookup(&anthropic_family_key(family, "MODEL_DESCRIPTION")))
}

fn resolve_family_capabilities<F>(
    provider: ProviderId,
    family: ModelFamily,
    env_lookup: &mut F,
) -> Option<Vec<ModelCapability>>
where
    F: FnMut(&str) -> Option<String>,
{
    let raw = provider_family_key(provider, family, "MODEL_SUPPORTED_CAPABILITIES")
        .and_then(|key| env_lookup(&key))
        .or_else(|| {
            env_lookup(&anthropic_family_key(
                family,
                "MODEL_SUPPORTED_CAPABILITIES",
            ))
        })?;
    let capabilities = raw
        .split(',')
        .filter_map(ModelCapability::parse)
        .collect::<Vec<_>>();
    Some(capabilities)
}

fn family_model_is_configured<F>(
    provider: ProviderId,
    family: ModelFamily,
    env_lookup: &mut F,
) -> bool
where
    F: FnMut(&str) -> Option<String>,
{
    provider_family_key(provider, family, "MODEL")
        .and_then(|key| env_lookup(&key))
        .or_else(|| env_lookup(&anthropic_family_key(family, "MODEL")))
        .is_some()
}

fn provider_family_key(provider: ProviderId, family: ModelFamily, suffix: &str) -> Option<String> {
    match provider {
        ProviderId::OpenAi => Some(format!("OPENAI_DEFAULT_{}_{}", family.env_name(), suffix)),
        _ => None,
    }
}

fn anthropic_family_key(family: ModelFamily, suffix: &str) -> String {
    format!("ANTHROPIC_DEFAULT_{}_{}", family.env_name(), suffix)
}

fn capitalize_family(family: ModelFamily) -> &'static str {
    match family {
        ModelFamily::Opus => "Opus",
        ModelFamily::Sonnet => "Sonnet",
        ModelFamily::Haiku => "Haiku",
    }
}

fn default_capabilities(provider: ProviderId, model: &str) -> Vec<ModelCapability> {
    match provider {
        ProviderId::OpenAi => vec![ModelCapability::Effort],
        ProviderId::Anthropic => {
            if model.to_ascii_lowercase().contains("claude") {
                vec![
                    ModelCapability::Thinking,
                    ModelCapability::InterleavedThinking,
                ]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

fn parse_model_setting(setting: &str) -> ParsedModelSetting<'_> {
    let trimmed = setting.trim();
    let lower = trimmed.to_ascii_lowercase();
    let (base, suffix) = lower
        .strip_suffix("[1m]")
        .map_or((lower.as_str(), ""), |base| (base.trim(), "[1m]"));
    let family = match base {
        "best" | "opus" | "opusplan" => Some(ModelFamily::Opus),
        "sonnet" => Some(ModelFamily::Sonnet),
        "haiku" => Some(ModelFamily::Haiku),
        _ => None,
    };
    ParsedModelSetting {
        setting: trimmed,
        family,
        suffix,
    }
}

fn model_family_for_string(model: &str) -> Option<ModelFamily> {
    let lower = model.to_ascii_lowercase();
    if lower.contains("haiku") {
        Some(ModelFamily::Haiku)
    } else if lower.contains("opus") {
        Some(ModelFamily::Opus)
    } else if lower.contains("sonnet") {
        Some(ModelFamily::Sonnet)
    } else {
        None
    }
}

fn marketing_name_for_model(model: &str) -> Option<String> {
    let lower = model.to_ascii_lowercase();
    let has_1m = lower.contains("[1m]");
    let canonical = canonical_model_name(&lower);
    let label = match canonical.as_str() {
        "claude-opus-4-7" => context_label("Opus 4.7", has_1m),
        "claude-opus-4-6" => context_label("Opus 4.6", has_1m),
        "claude-opus-4-5" => "Opus 4.5".to_string(),
        "claude-opus-4-1" => "Opus 4.1".to_string(),
        "claude-opus-4" => "Opus 4".to_string(),
        "claude-sonnet-4-6" => context_label("Sonnet 4.6", has_1m),
        "claude-sonnet-4-5" => context_label("Sonnet 4.5", has_1m),
        "claude-sonnet-4" => context_label("Sonnet 4", has_1m),
        "claude-3-7-sonnet" => "Claude 3.7 Sonnet".to_string(),
        "claude-3-5-sonnet" => "Claude 3.5 Sonnet".to_string(),
        "claude-haiku-4-5" => "Haiku 4.5".to_string(),
        "claude-3-5-haiku" => "Claude 3.5 Haiku".to_string(),
        _ => return None,
    };
    Some(label)
}

pub fn canonical_model_name(model: &str) -> String {
    for known in [
        "claude-opus-4-7",
        "claude-opus-4-6",
        "claude-opus-4-5",
        "claude-opus-4-1",
        "claude-opus-4",
        "claude-sonnet-4-6",
        "claude-sonnet-4-5",
        "claude-sonnet-4",
        "claude-haiku-4-5",
        "claude-3-7-sonnet",
        "claude-3-5-sonnet",
        "claude-3-5-haiku",
        "claude-3-opus",
        "claude-3-sonnet",
        "claude-3-haiku",
    ] {
        if model.contains(known) {
            return known.to_string();
        }
    }
    model.to_string()
}

fn context_label(label: &str, has_1m: bool) -> String {
    if has_1m {
        format!("{label} (with 1M context)")
    } else {
        label.to_string()
    }
}

fn known_model_family(model: &str) -> Option<ModelFamily> {
    let canonical = canonical_model_name(&model.to_ascii_lowercase());
    if matches!(
        canonical.as_str(),
        "claude-sonnet-4-6"
            | "claude-sonnet-4-5"
            | "claude-sonnet-4"
            | "claude-3-7-sonnet"
            | "claude-3-5-sonnet"
    ) {
        Some(ModelFamily::Sonnet)
    } else if canonical.starts_with("claude-opus-4") {
        Some(ModelFamily::Opus)
    } else if canonical.starts_with("claude-haiku") || canonical == "claude-3-5-haiku" {
        Some(ModelFamily::Haiku)
    } else {
        None
    }
}

fn builtin_anthropic_family_model(family: ModelFamily) -> &'static str {
    match family {
        ModelFamily::Opus => DEFAULT_ANTHROPIC_OPUS_MODEL,
        ModelFamily::Sonnet => DEFAULT_ANTHROPIC_SONNET_MODEL,
        ModelFamily::Haiku => DEFAULT_ANTHROPIC_HAIKU_MODEL,
    }
}

fn builtin_openai_family_model(family: ModelFamily) -> &'static str {
    match family {
        ModelFamily::Opus => DEFAULT_OPENAI_OPUS_MODEL,
        ModelFamily::Sonnet => DEFAULT_OPENAI_SONNET_MODEL,
        ModelFamily::Haiku => DEFAULT_OPENAI_HAIKU_MODEL,
    }
}

struct ParsedModelSetting<'a> {
    setting: &'a str,
    family: Option<ModelFamily>,
    suffix: &'static str,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn aliases_resolve_to_current_anthropic_defaults() {
        assert_eq!(
            resolve_provider_model(ProviderId::Anthropic, Some("sonnet"), |_| None).request_model,
            "claude-sonnet-4-6"
        );
        assert_eq!(
            resolve_provider_model(ProviderId::Anthropic, Some("opus"), |_| None).request_model,
            "claude-opus-4-7"
        );
        assert_eq!(
            resolve_provider_model(ProviderId::Anthropic, Some("best"), |_| None).request_model,
            "claude-opus-4-7"
        );
        assert_eq!(
            resolve_provider_model(ProviderId::Anthropic, Some("haiku"), |_| None).request_model,
            "claude-haiku-4-5-20251001"
        );
    }

    #[test]
    fn openai_uses_provider_specific_family_defaults() {
        assert_eq!(
            resolve_provider_model(ProviderId::OpenAi, Some("sonnet"), |_| None).request_model,
            "gpt-4o"
        );
        assert_eq!(
            resolve_provider_model(ProviderId::OpenAi, Some("opus"), |_| None).request_model,
            "o3"
        );
        assert_eq!(
            resolve_provider_model(ProviderId::OpenAi, Some("haiku"), |_| None).request_model,
            "gpt-4o-mini"
        );
    }

    #[test]
    fn openai_env_overrides_win_before_anthropic_fallback() {
        let env = HashMap::from([
            (
                "OPENAI_DEFAULT_SONNET_MODEL".to_string(),
                "gpt-custom-sonnet".to_string(),
            ),
            (
                "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
                "claude-custom-sonnet".to_string(),
            ),
        ]);

        assert_eq!(
            resolve_provider_model(ProviderId::OpenAi, Some("sonnet"), |key| env
                .get(key)
                .cloned())
            .request_model,
            "gpt-custom-sonnet"
        );
    }

    #[test]
    fn family_capabilities_are_parsed_from_provider_env() {
        let env = HashMap::from([
            ("OPENAI_DEFAULT_OPUS_MODEL".to_string(), "o3".to_string()),
            (
                "OPENAI_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES".to_string(),
                "effort,thinking,unknown".to_string(),
            ),
        ]);

        assert_eq!(
            resolve_provider_model(ProviderId::OpenAi, Some("opus"), |key| env
                .get(key)
                .cloned())
            .capabilities,
            vec![ModelCapability::Effort, ModelCapability::Thinking]
        );
    }

    #[test]
    fn small_fast_prefers_provider_env_and_falls_back_to_haiku() {
        let env = HashMap::from([
            (
                "OPENAI_SMALL_FAST_MODEL".to_string(),
                "gpt-fast".to_string(),
            ),
            (
                "OPENAI_DEFAULT_HAIKU_MODEL".to_string(),
                "gpt-haiku".to_string(),
            ),
        ]);
        assert_eq!(
            resolve_small_fast_model(ProviderId::OpenAi, |key| env.get(key).cloned()).request_model,
            "gpt-fast"
        );

        let env = HashMap::from([(
            "ANTHROPIC_SMALL_FAST_MODEL".to_string(),
            "claude-fast[1m]".to_string(),
        )]);
        assert_eq!(
            resolve_small_fast_model(ProviderId::Anthropic, |key| env.get(key).cloned())
                .request_model,
            "claude-fast"
        );

        assert_eq!(
            resolve_small_fast_model(ProviderId::Anthropic, |_| None).request_model,
            "claude-haiku-4-5-20251001"
        );
    }

    #[test]
    fn known_model_option_uses_marketing_label_and_upgrade_hint() {
        let option = known_model_option(
            ProviderId::Anthropic,
            "claude-sonnet-4-5-20250929[1m]",
            |_| None,
        )
        .expect("known model option");

        assert_eq!(option.label, "Sonnet 4.5 (with 1M context)");
        assert_eq!(
            option.description,
            "Newer version available - select Sonnet for Sonnet 4.6"
        );

        let option =
            known_model_option(ProviderId::Anthropic, "claude-sonnet-4-6-20260401", |_| {
                None
            })
            .expect("known current model option");

        assert_eq!(option.label, "Sonnet 4.6");
        assert_eq!(option.description, "claude-sonnet-4-6-20260401");
    }

    #[test]
    fn model_capabilities_anthropic_known_models() {
        let opus47 = model_capabilities("claude-opus-4-7", ProviderId::Anthropic);
        assert_eq!(opus47.context_window, 1_000_000);
        assert_eq!(opus47.max_output_tokens, 64_000);
        assert_eq!(opus47.max_output_tokens_upper_limit, 128_000);
        assert!(opus47.supports_thinking);
        assert!(opus47.supports_vision);
        assert!(opus47.supports_streaming);

        let sonnet46 = model_capabilities("claude-sonnet-4-6-20250514", ProviderId::Anthropic);
        assert_eq!(sonnet46.context_window, 1_000_000);
        assert_eq!(sonnet46.max_output_tokens, 32_000);
        assert_eq!(sonnet46.max_output_tokens_upper_limit, 128_000);
        assert!(sonnet46.supports_thinking);
        assert!(sonnet46.supports_vision);

        let haiku = model_capabilities("claude-haiku-4-5-20251001", ProviderId::Anthropic);
        assert_eq!(haiku.context_window, 200_000);
        assert_eq!(haiku.max_output_tokens, 32_000);
        assert!(haiku.supports_thinking);
        assert!(haiku.supports_vision);

        let old_sonnet = model_capabilities("claude-3-5-sonnet-20241022", ProviderId::Anthropic);
        assert_eq!(old_sonnet.context_window, 200_000);
        assert_eq!(old_sonnet.max_output_tokens, 8_192);
        assert!(!old_sonnet.supports_thinking);
        assert!(old_sonnet.supports_vision);
    }

    #[test]
    fn model_capabilities_openai_known_models() {
        let o3 = model_capabilities("o3", ProviderId::OpenAi);
        assert_eq!(o3.context_window, 200_000);
        assert_eq!(o3.max_output_tokens, 100_000);
        assert!(!o3.supports_thinking);
        assert!(o3.supports_vision);
        assert!(o3.supports_streaming);

        let gpt4o = model_capabilities("gpt-4o", ProviderId::OpenAi);
        assert_eq!(gpt4o.context_window, 128_000);
        assert_eq!(gpt4o.max_output_tokens, 16_384);
        assert!(gpt4o.supports_vision);
    }

    #[test]
    fn model_capabilities_unknown_model_uses_fallback() {
        let unknown = model_capabilities("custom-model-v2", ProviderId::Anthropic);
        assert_eq!(unknown.context_window, 200_000);
        assert_eq!(unknown.max_output_tokens, 32_000);
        assert_eq!(unknown.max_output_tokens_upper_limit, 64_000);
        assert!(!unknown.supports_thinking);
        assert!(!unknown.supports_vision);
        assert!(unknown.supports_streaming);

        let unknown_openai = model_capabilities("my-custom-llm", ProviderId::OpenAi);
        assert_eq!(unknown_openai.context_window, 128_000);
        assert_eq!(unknown_openai.max_output_tokens, 16_384);
        assert!(!unknown_openai.supports_vision);
    }

    #[test]
    fn disabled_providers_resolve_without_panic() {
        for provider in ProviderId::DISABLED {
            let resolution = resolve_provider_model(provider, None, |_| None);
            assert_eq!(resolution.provider, provider);
            assert!(resolution.family.is_some());
            assert!(!resolution.request_model.is_empty());
        }
    }

    #[test]
    fn disabled_providers_passthrough_explicit_model() {
        for provider in ProviderId::DISABLED {
            let resolution = resolve_provider_model(provider, Some("my-custom-model"), |_| None);
            assert_eq!(resolution.provider, provider);
            assert_eq!(resolution.request_model, "my-custom-model");
            assert!(resolution.family.is_none());
        }
    }

    #[test]
    fn disabled_providers_have_no_default_capabilities() {
        for provider in ProviderId::DISABLED {
            let resolution = resolve_provider_model(provider, Some("unknown-model"), |_| None);
            assert!(
                resolution.capabilities.is_empty(),
                "disabled provider '{provider}' should not advertise capabilities for unknown models"
            );
        }
    }
}
