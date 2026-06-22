use anyhow::{anyhow, Result};

use crate::{
    config::{AgentProvider, HarnessConfig},
    types::AgentModelResponse,
};

pub fn complete(config: &HarnessConfig, prompt: &str) -> Result<Option<AgentModelResponse>> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(anyhow!("agent prompt cannot be empty"));
    }

    match config.agent_provider {
        AgentProvider::DeterministicTest => Ok(Some(deterministic_response(config, prompt))),
        AgentProvider::OpenAiCompatibleHttp | AgentProvider::LocalHttp => Ok(None),
    }
}

fn deterministic_response(config: &HarnessConfig, prompt: &str) -> AgentModelResponse {
    AgentModelResponse {
        ok: true,
        provider: AgentProvider::DeterministicTest.as_str().to_string(),
        model: config.agent_model.clone(),
        prompt: prompt.to_string(),
        text: format!("deterministic-agent: {}", normalize_prompt(prompt)),
    }
}

fn normalize_prompt(prompt: &str) -> String {
    prompt.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_agent_normalizes_prompt_without_network() {
        let config = HarnessConfig::default();

        let response = complete(&config, " inspect   the battery ")
            .unwrap()
            .unwrap();

        assert_eq!(response.provider, "deterministic-test");
        assert_eq!(response.model, "deterministic-test");
        assert_eq!(response.text, "deterministic-agent: inspect the battery");
    }

    #[test]
    fn hosted_provider_does_not_make_network_call() {
        let config = HarnessConfig {
            agent_provider: AgentProvider::OpenAiCompatibleHttp,
            agent_base_url: Some("https://example.test/v1".to_string()),
            agent_api_key: Some("secret".to_string()),
            ..HarnessConfig::default()
        };

        assert!(complete(&config, "inspect the battery").unwrap().is_none());
    }
}
