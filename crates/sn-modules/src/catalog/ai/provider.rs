use serde::Serialize;

pub const CODES: [&str; 7] = [
    "groq",
    "gemini",
    "deepseek",
    "openrouter",
    "openai",
    "anthropic",
    "custom",
];

pub const LABELS: [&str; 7] = [
    "Groq",
    "Google Gemini",
    "DeepSeek",
    "OpenRouter",
    "OpenAI",
    "Anthropic (Claude)",
    "Свой адрес, совместимый с OpenAI",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Groq,
    Gemini,
    DeepSeek,
    OpenRouter,
    OpenAi,
    Anthropic,
    Custom,
}

impl Provider {
    pub const ALL: [Provider; 7] = [
        Provider::Groq,
        Provider::Gemini,
        Provider::DeepSeek,
        Provider::OpenRouter,
        Provider::OpenAi,
        Provider::Anthropic,
        Provider::Custom,
    ];

    pub const fn code(self) -> &'static str {
        CODES[self as usize]
    }

    pub const fn label(self) -> &'static str {
        LABELS[self as usize]
    }

    pub fn parse(code: &str) -> Option<Self> {
        let code = code.trim();
        Provider::ALL.into_iter().find(|p| p.code() == code)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Provider::Groq => "Groq",
            Provider::Gemini => "Google Gemini",
            Provider::DeepSeek => "DeepSeek",
            Provider::OpenRouter => "OpenRouter",
            Provider::OpenAi => "OpenAI",
            Provider::Anthropic => "Anthropic",
            Provider::Custom => "Сервер API",
        }
    }

    pub const fn base_url(self) -> Option<&'static str> {
        match self {
            Provider::Groq => Some("https://api.groq.com/openai/v1"),
            Provider::Gemini => Some("https://generativelanguage.googleapis.com/v1beta/openai"),
            Provider::DeepSeek => Some("https://api.deepseek.com"),
            Provider::OpenRouter => Some("https://openrouter.ai/api/v1"),
            Provider::OpenAi => Some("https://api.openai.com/v1"),
            Provider::Anthropic => Some("https://api.anthropic.com/v1"),
            Provider::Custom => None,
        }
    }

    pub const fn default_model(self) -> Option<&'static str> {
        match self {
            Provider::Groq => Some("openai/gpt-oss-120b"),
            Provider::Gemini => Some("gemini-3.5-flash-lite"),
            Provider::DeepSeek => Some("deepseek-flash"),
            Provider::OpenRouter => Some("openai/gpt-oss-120b"),
            Provider::OpenAi => Some("gpt-6-luna"),
            Provider::Anthropic => Some("claude-haiku-5-5"),
            Provider::Custom => None,
        }
    }

    pub const fn keys_page(self) -> Option<&'static str> {
        match self {
            Provider::Groq => Some("console.groq.com/keys"),
            Provider::Gemini => Some("aistudio.google.com/apikey"),
            Provider::DeepSeek => Some("platform.deepseek.com/api_keys"),
            Provider::OpenRouter => Some("openrouter.ai/settings/keys"),
            Provider::OpenAi => Some("platform.openai.com/api-keys"),
            Provider::Anthropic => Some("platform.claude.com/settings/keys"),
            Provider::Custom => None,
        }
    }

    pub const fn blocked_by_country(self) -> bool {
        matches!(
            self,
            Provider::Groq | Provider::Gemini | Provider::OpenAi | Provider::Anthropic
        )
    }

    pub fn model_id(self, raw: &str) -> String {
        let model = raw.trim();
        match self {
            Provider::Gemini => model.strip_prefix("models/").unwrap_or(model).to_string(),
            _ => model.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn провайдеры_читаются_по_коду_и_знают_свои_умолчания() {
        for provider in Provider::ALL {
            assert_eq!(Provider::parse(provider.code()), Some(provider));
            assert_eq!(serde_json::to_value(provider).unwrap(), provider.code());
            assert!(!provider.label().is_empty());
            let custom = provider == Provider::Custom;
            assert_eq!(provider.base_url().is_none(), custom);
            assert_eq!(provider.default_model().is_none(), custom);
            assert_eq!(provider.keys_page().is_none(), custom);
            assert!(
                provider
                    .base_url()
                    .is_none_or(|url| url.starts_with("https://"))
            );
        }
        assert_eq!(Provider::parse(" gemini "), Some(Provider::Gemini));
        assert_eq!(Provider::parse("mistral"), None);
        assert_eq!(
            Provider::Gemini.model_id("models/gemini-3.8-flash"),
            "gemini-3.8-flash"
        );
        assert_eq!(Provider::OpenRouter.model_id(" models/x "), "models/x");
    }
}
