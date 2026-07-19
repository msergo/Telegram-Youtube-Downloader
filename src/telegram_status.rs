use log::{error, warn};

const TELEGRAM_API_BASE_URL: &str = "https://api.telegram.org";

pub(crate) struct TelegramStatusMessage {
    client: reqwest::Client,
    api_base_url: String,
    bot_token: String,
    chat_id: i64,
    message_id: Option<i64>,
}

#[derive(serde::Serialize)]
struct SendMessageRequest<'a> {
    chat_id: i64,
    text: &'a str,
}

#[derive(serde::Serialize)]
struct EditMessageTextRequest<'a> {
    chat_id: i64,
    message_id: i64,
    text: &'a str,
}

#[derive(serde::Serialize)]
struct DeleteMessageRequest {
    chat_id: i64,
    message_id: i64,
}

#[derive(serde::Deserialize)]
struct TelegramApiResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(serde::Deserialize)]
struct SentMessage {
    message_id: i64,
}

impl TelegramStatusMessage {
    pub(crate) async fn create(chat_id: i64, bot_token: &str, initial_text: &str) -> Self {
        Self::create_with_client_and_base_url(
            reqwest::Client::new(),
            TELEGRAM_API_BASE_URL,
            chat_id,
            bot_token,
            initial_text,
        )
        .await
    }

    #[cfg(test)]
    async fn create_with_base_url(
        api_base_url: &str,
        chat_id: i64,
        bot_token: &str,
        initial_text: &str,
    ) -> Self {
        Self::create_with_client_and_base_url(
            reqwest::Client::new(),
            api_base_url,
            chat_id,
            bot_token,
            initial_text,
        )
        .await
    }

    async fn create_with_client_and_base_url(
        client: reqwest::Client,
        api_base_url: &str,
        chat_id: i64,
        bot_token: &str,
        initial_text: &str,
    ) -> Self {
        let mut status = Self {
            client,
            api_base_url: api_base_url.trim_end_matches('/').to_string(),
            bot_token: bot_token.to_string(),
            chat_id,
            message_id: None,
        };

        let request = SendMessageRequest {
            chat_id,
            text: initial_text,
        };

        let response = match status
            .client
            .post(status.endpoint("sendMessage"))
            .json(&request)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                error!("Failed to create Telegram status message: {}", error);
                return status;
            }
        };

        if !response.status().is_success() {
            warn!(
                "Telegram status message creation returned HTTP status {}",
                response.status()
            );
            return status;
        }

        let body = match response.json::<TelegramApiResponse<SentMessage>>().await {
            Ok(body) => body,
            Err(error) => {
                error!(
                    "Failed to decode Telegram status message response: {}",
                    error
                );
                return status;
            }
        };

        if !body.ok {
            warn!(
                "Telegram status message creation failed: {}",
                body.description
                    .as_deref()
                    .unwrap_or("missing API description")
            );
            return status;
        }

        let Some(sent_message) = body.result else {
            warn!("Telegram status message creation response did not include a result");
            return status;
        };

        status.message_id = Some(sent_message.message_id);
        status
    }

    pub(crate) async fn update(&self, text: &str) {
        let Some(message_id) = self.message_id else {
            return;
        };

        let request = EditMessageTextRequest {
            chat_id: self.chat_id,
            message_id,
            text,
        };

        let response = match self
            .client
            .post(self.endpoint("editMessageText"))
            .json(&request)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                error!("Failed to update Telegram status message: {}", error);
                return;
            }
        };

        if !response.status().is_success() {
            warn!(
                "Telegram status message update returned HTTP status {}",
                response.status()
            );
            return;
        }

        let body = match response
            .json::<TelegramApiResponse<serde_json::Value>>()
            .await
        {
            Ok(body) => body,
            Err(error) => {
                error!(
                    "Failed to decode Telegram status update response: {}",
                    error
                );
                return;
            }
        };

        if !body.ok {
            warn!(
                "Telegram status message update failed: {}",
                body.description
                    .as_deref()
                    .unwrap_or("missing API description")
            );
            return;
        }

        if body.result.is_none() {
            warn!("Telegram status message update response did not include a result");
        }
    }

    pub(crate) async fn delete(&self) {
        let Some(message_id) = self.message_id else {
            return;
        };

        let request = DeleteMessageRequest {
            chat_id: self.chat_id,
            message_id,
        };

        let response = match self
            .client
            .post(self.endpoint("deleteMessage"))
            .json(&request)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                error!("Failed to delete Telegram status message: {}", error);
                return;
            }
        };

        if !response.status().is_success() {
            warn!(
                "Telegram status message deletion returned HTTP status {}",
                response.status()
            );
            return;
        }

        let body = match response.json::<TelegramApiResponse<bool>>().await {
            Ok(body) => body,
            Err(error) => {
                error!(
                    "Failed to decode Telegram status deletion response: {}",
                    error
                );
                return;
            }
        };

        if !body.ok {
            warn!(
                "Telegram status message deletion failed: {}",
                body.description
                    .as_deref()
                    .unwrap_or("missing API description")
            );
            return;
        }

        if body.result != Some(true) {
            warn!("Telegram status message deletion response did not confirm deletion");
        }
    }

    fn endpoint(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.api_base_url, self.bot_token, method)
    }
}

#[cfg(test)]
mod tests {
    use super::TelegramStatusMessage;
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const CHAT_ID: i64 = 12345;
    const TOKEN: &str = "TEST_TOKEN";

    fn successful_message(message_id: i64) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "result": {
                "message_id": message_id
            }
        }))
    }

    fn successful_edit() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "result": {
                "message_id": 42
            }
        }))
    }

    fn successful_delete() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "result": true
        }))
    }

    async fn create_status(server: &MockServer) -> TelegramStatusMessage {
        TelegramStatusMessage::create_with_base_url(&server.uri(), CHAT_ID, TOKEN, "Starting...")
            .await
    }

    #[tokio::test]
    async fn send_message_stores_returned_message_id() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/sendMessage"))
            .and(body_json(json!({
                "chat_id": CHAT_ID,
                "text": "Starting..."
            })))
            .respond_with(successful_message(42))
            .expect(1)
            .mount(&server)
            .await;

        let status = create_status(&server).await;

        assert_eq!(status.message_id, Some(42));
    }

    #[tokio::test]
    async fn update_posts_edit_message_text_with_stored_message_id() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/sendMessage"))
            .respond_with(successful_message(42))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/editMessageText"))
            .and(body_json(json!({
                "chat_id": CHAT_ID,
                "message_id": 42,
                "text": "Downloading and converting..."
            })))
            .respond_with(successful_edit())
            .expect(1)
            .mount(&server)
            .await;

        let status = create_status(&server).await;
        status.update("Downloading and converting...").await;
    }

    #[tokio::test]
    async fn stage_sequence_reuses_one_status_message() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/sendMessage"))
            .respond_with(successful_message(42))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/editMessageText"))
            .respond_with(successful_edit())
            .expect(2)
            .mount(&server)
            .await;

        let status = create_status(&server).await;
        status.update("Downloading and converting...").await;
        status.update("Download completed").await;

        let requests = server.received_requests().await.unwrap();
        let send_message_count = requests
            .iter()
            .filter(|request| request.url.path() == "/botTEST_TOKEN/sendMessage")
            .count();
        let edit_requests = requests
            .iter()
            .filter(|request| request.url.path() == "/botTEST_TOKEN/editMessageText")
            .collect::<Vec<_>>();

        assert_eq!(send_message_count, 1);
        assert_eq!(edit_requests.len(), 2);
        for request in edit_requests {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["message_id"], 42);
        }
    }

    #[tokio::test]
    async fn creation_http_failure_disables_later_updates() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/sendMessage"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        let status = create_status(&server).await;
        status.update("Download failed").await;

        assert_eq!(status.message_id, None);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn creation_api_failure_disables_later_updates() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "description": "Bad Request"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let status = create_status(&server).await;
        status.update("Download failed").await;

        assert_eq!(status.message_id, None);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn malformed_creation_response_disables_later_updates() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .expect(1)
            .mount(&server)
            .await;

        let status = create_status(&server).await;
        status.update("Download failed").await;

        assert_eq!(status.message_id, None);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn missing_result_creation_response_disables_later_updates() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true
            })))
            .expect(1)
            .mount(&server)
            .await;

        let status = create_status(&server).await;
        status.update("Download failed").await;

        assert_eq!(status.message_id, None);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn failed_edit_does_not_disable_later_terminal_update() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/sendMessage"))
            .respond_with(successful_message(42))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/editMessageText"))
            .and(body_json(json!({
                "chat_id": CHAT_ID,
                "message_id": 42,
                "text": "Downloading and converting..."
            })))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/editMessageText"))
            .and(body_json(json!({
                "chat_id": CHAT_ID,
                "message_id": 42,
                "text": "Download completed"
            })))
            .respond_with(successful_edit())
            .expect(1)
            .mount(&server)
            .await;

        let status = create_status(&server).await;
        status.update("Downloading and converting...").await;
        status.update("Download completed").await;

        let edit_count = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.url.path() == "/botTEST_TOKEN/editMessageText")
            .count();

        assert_eq!(edit_count, 2);
    }

    #[tokio::test]
    async fn terminal_edit_failure_returns_normally() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/sendMessage"))
            .respond_with(successful_message(42))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/editMessageText"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        let status = create_status(&server).await;
        status.update("Download completed").await;
    }

    #[tokio::test]
    async fn delete_posts_delete_message_with_stored_message_id() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/sendMessage"))
            .respond_with(successful_message(42))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/deleteMessage"))
            .and(body_json(json!({
                "chat_id": CHAT_ID,
                "message_id": 42
            })))
            .respond_with(successful_delete())
            .expect(1)
            .mount(&server)
            .await;

        let status = create_status(&server).await;
        status.delete().await;
    }

    #[tokio::test]
    async fn delete_on_disabled_handle_sends_no_http_request() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/sendMessage"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        let status = create_status(&server).await;
        status.delete().await;

        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn delete_failure_returns_normally() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/sendMessage"))
            .respond_with(successful_message(42))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/deleteMessage"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        let status = create_status(&server).await;
        status.delete().await;
    }

    #[tokio::test]
    async fn independent_handles_use_their_own_message_ids() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/sendMessage"))
            .and(body_json(json!({
                "chat_id": CHAT_ID,
                "text": "First"
            })))
            .respond_with(successful_message(10))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/sendMessage"))
            .and(body_json(json!({
                "chat_id": CHAT_ID,
                "text": "Second"
            })))
            .respond_with(successful_message(20))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/botTEST_TOKEN/editMessageText"))
            .respond_with(successful_edit())
            .expect(2)
            .mount(&server)
            .await;

        let first =
            TelegramStatusMessage::create_with_base_url(&server.uri(), CHAT_ID, TOKEN, "First")
                .await;
        let second =
            TelegramStatusMessage::create_with_base_url(&server.uri(), CHAT_ID, TOKEN, "Second")
                .await;

        first.update("Download completed").await;
        second.update("Download failed").await;

        let mut message_ids = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.url.path() == "/botTEST_TOKEN/editMessageText")
            .map(|request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                body["message_id"].as_i64().unwrap()
            })
            .collect::<Vec<_>>();

        message_ids.sort_unstable();
        assert_eq!(message_ids, vec![10, 20]);
    }
}
