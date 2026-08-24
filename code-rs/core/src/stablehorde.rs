use std::time::Duration;

use code_protocol::models::ContentItem;
use code_protocol::models::ResponseItem;
use reqwest::Url;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::ModelProviderInfo;
use crate::client_common::Prompt;
use crate::client_common::ResponseEvent;
use crate::client_common::ResponseStream;
use crate::error::CodexErr;
use crate::error::Result;
use crate::model_family::ModelFamily;
use crate::model_provider_info::STABLEHORDE_API_BASE_URL;
use crate::model_provider_info::STABLEHORDE_V2_API_BASE_URL;

const STABLEHORDE_POLL_INTERVAL: Duration = Duration::from_secs(1);
const STABLEHORDE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const STABLEHORDE_GENERATION_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Deserialize)]
struct StableHordeRequestAccepted {
    id: String,
}

#[derive(Debug, Deserialize)]
struct StableHordeGeneration {
    text: String,
    #[serde(default)]
    model: String,
}

#[derive(Debug, Deserialize)]
struct StableHordeRequestStatus {
    #[serde(default)]
    done: bool,
    #[serde(default)]
    faulted: bool,
    #[serde(default = "default_true")]
    is_possible: bool,
    #[serde(default)]
    generations: Vec<StableHordeGeneration>,
}

fn default_true() -> bool {
    true
}

pub(crate) async fn stream_stablehorde_v2(
    prompt: &Prompt,
    model_family: &ModelFamily,
    model_slug: &str,
    client: &reqwest::Client,
    provider: &ModelProviderInfo,
    responses_originator_header: &str,
    v1_error: String,
) -> Result<ResponseStream> {
    let api_key = provider.api_key()?.ok_or_else(|| {
        CodexErr::ServerError("Stable Horde API key resolution returned no key".to_owned())
    })?;
    let base_url = stablehorde_v2_base_url(provider)?;
    let submit_url = endpoint_url(&base_url, "generate/text/async")?;
    let max_context_length = model_family.context_window.unwrap_or(8_192).min(32_768);
    let max_length = model_family.max_output_tokens.unwrap_or(1_024).min(2_048);
    let payload = json!({
        "prompt": stablehorde_prompt(prompt, model_family)?,
        "models": [model_slug],
        "params": {
            "max_context_length": max_context_length,
            "max_length": max_length,
            "stop_sequence": [],
        },
        "slow_workers": true,
        "validated_backends": true,
    });
    let response = client
        .post(submit_url)
        .header("apikey", api_key)
        .header("Client-Agent", responses_originator_header)
        .timeout(STABLEHORDE_REQUEST_TIMEOUT)
        .json(&payload)
        .send()
        .await
        .map_err(|error| combined_error(&v1_error, &error.to_string()))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(combined_error(
            &v1_error,
            &format!("submission returned HTTP {status}: {body}"),
        ));
    }
    let accepted = response
        .json::<StableHordeRequestAccepted>()
        .await
        .map_err(|error| combined_error(&v1_error, &format!("invalid submission response: {error}")))?;
    let status_url = endpoint_url(&base_url, &format!("generate/text/status/{}", accepted.id))?;
    let request_id = accepted.id;
    let model_slug = model_slug.to_owned();
    let client = client.clone();
    let originator = responses_originator_header.to_owned();
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent>>(16);

    tokio::spawn(async move {
        let started = Instant::now();
        loop {
            if tx_event.is_closed() {
                cancel_request(&client, &status_url, &originator).await;
                return;
            }
            if started.elapsed() >= STABLEHORDE_GENERATION_TIMEOUT {
                cancel_request(&client, &status_url, &originator).await;
                let _ = tx_event
                    .send(Err(combined_error(
                        &v1_error,
                        "v2 generation timed out after 300 seconds",
                    )))
                    .await;
                return;
            }

            let status_response = match client
                .get(status_url.clone())
                .header("Client-Agent", &originator)
                .timeout(STABLEHORDE_REQUEST_TIMEOUT)
                .send()
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    cancel_request(&client, &status_url, &originator).await;
                    let _ = tx_event
                        .send(Err(combined_error(&v1_error, &error.to_string())))
                        .await;
                    return;
                }
            };
            if !status_response.status().is_success() {
                let status = status_response.status();
                let body = status_response.text().await.unwrap_or_default();
                cancel_request(&client, &status_url, &originator).await;
                let _ = tx_event
                    .send(Err(combined_error(
                        &v1_error,
                        &format!("status returned HTTP {status}: {body}"),
                    )))
                    .await;
                return;
            }
            let status = match status_response.json::<StableHordeRequestStatus>().await {
                Ok(status) => status,
                Err(error) => {
                    cancel_request(&client, &status_url, &originator).await;
                    let _ = tx_event
                        .send(Err(combined_error(
                            &v1_error,
                            &format!("invalid status response: {error}"),
                        )))
                        .await;
                    return;
                }
            };
            if status.faulted || !status.is_possible {
                cancel_request(&client, &status_url, &originator).await;
                let reason = if status.faulted {
                    "v2 generation faulted"
                } else {
                    "v2 generation is not possible with the available workers"
                };
                let _ = tx_event
                    .send(Err(combined_error(&v1_error, reason)))
                    .await;
                return;
            }
            if status.done {
                let Some(generation) = status.generations.into_iter().next() else {
                    let _ = tx_event
                        .send(Err(combined_error(
                            &v1_error,
                            "v2 generation completed without text",
                        )))
                        .await;
                    return;
                };
                let response_model = (!generation.model.trim().is_empty())
                    .then_some(generation.model)
                    .or_else(|| Some(model_slug.clone()));
                let events = [
                    ResponseEvent::Created {
                        response_id: Some(request_id.clone()),
                        response_model,
                    },
                    ResponseEvent::OutputItemDone {
                        item: ResponseItem::Message {
                            id: Some(request_id.clone()),
                            role: "assistant".to_owned(),
                            content: vec![ContentItem::OutputText {
                                text: generation.text,
                            }],
                            end_turn: None,
                            phase: None,
                        },
                        sequence_number: None,
                        output_index: None,
                    },
                    ResponseEvent::Completed {
                        response_id: request_id,
                        token_usage: None,
                    },
                ];
                for event in events {
                    if tx_event.send(Ok(event)).await.is_err() {
                        return;
                    }
                }
                return;
            }

            tokio::time::sleep(STABLEHORDE_POLL_INTERVAL).await;
        }
    });

    Ok(ResponseStream {
        pending_events: Default::default(),
        rx_event,
    })
}

fn stablehorde_prompt(prompt: &Prompt, model_family: &ModelFamily) -> Result<String> {
    let input = serde_json::to_string(&prompt.get_formatted_input())?;
    Ok(format!(
        "System instructions:\n{}\n\nConversation items (JSON):\n{input}\n\nRespond as the assistant.",
        prompt.get_full_instructions(model_family)
    ))
}

fn stablehorde_v2_base_url(provider: &ModelProviderInfo) -> Result<String> {
    let configured_base = provider
        .base_url
        .as_deref()
        .unwrap_or(STABLEHORDE_API_BASE_URL)
        .trim_end_matches('/');
    if configured_base == STABLEHORDE_API_BASE_URL {
        return Ok(STABLEHORDE_V2_API_BASE_URL.to_owned());
    }
    if let Some(prefix) = configured_base.strip_suffix("/v1") {
        return Ok(format!("{prefix}/v2"));
    }
    Err(CodexErr::ServerError(format!(
        "cannot derive Stable Horde v2 endpoint from {configured_base}"
    )))
}

fn endpoint_url(base_url: &str, suffix: &str) -> Result<Url> {
    let mut url = Url::parse(base_url).map_err(|error| {
        CodexErr::ServerError(format!("invalid Stable Horde v2 base URL {base_url}: {error}"))
    })?;
    let base_path = url.path().trim_end_matches('/');
    url.set_path(&format!("{base_path}/{}", suffix.trim_start_matches('/')));
    Ok(url)
}

async fn cancel_request(client: &reqwest::Client, status_url: &Url, originator: &str) {
    if let Err(error) = client
        .delete(status_url.clone())
        .header("Client-Agent", originator)
        .timeout(STABLEHORDE_REQUEST_TIMEOUT)
        .send()
        .await
    {
        tracing::debug!("failed to cancel Stable Horde v2 request: {error}");
    }
}

fn combined_error(v1_error: &str, v2_error: &str) -> CodexErr {
    CodexErr::ServerError(format!(
        "Stable Horde request failed through both transports: v1 proxy: {v1_error}; v2 direct: {v2_error}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WireApi;

    fn provider(base_url: &str) -> ModelProviderInfo {
        ModelProviderInfo::direct_openai_compatible(
            "Stable Horde",
            base_url,
            None,
            WireApi::Chat,
        )
    }

    #[test]
    fn mock_v1_base_derives_matching_v2_base() {
        assert_eq!(
            stablehorde_v2_base_url(&provider("http://127.0.0.1:8080/v1"))
                .expect("derived v2 URL"),
            "http://127.0.0.1:8080/v2"
        );
    }

    #[test]
    fn public_proxy_base_uses_direct_ai_horde_api() {
        assert_eq!(
            stablehorde_v2_base_url(&provider(STABLEHORDE_API_BASE_URL))
                .expect("public v2 URL"),
            STABLEHORDE_V2_API_BASE_URL
        );
    }
}
