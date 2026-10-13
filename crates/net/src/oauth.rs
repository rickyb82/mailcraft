//! The OAuth broker over HTTPS: exchanges authorization codes and refreshes tokens at the
//! provider's token endpoint (`ehttp`, blocking: the engine's worker calls it).

use mailcraft_core::oauth::{ClientCredentials, OAuthBroker, Provider, Tokens, exchange_body, refresh_body};
use mailcraft_core::{Error, Result};

/// Talks to the provider's token endpoint.
#[derive(Default)]
pub struct HttpBroker;

impl HttpBroker {
    fn post(&self, url: &str, body: String, old_refresh: &str) -> Result<Tokens> {
        let mut req = ehttp::Request::post(url, body.into_bytes());
        req.headers.insert("Content-Type", "application/x-www-form-urlencoded");
        req.headers.insert("Accept", "application/json");
        let resp = ehttp::fetch_blocking(&req).map_err(|e| Error::Connect(format!("OAuth token endpoint: {e}")))?;
        let v: serde_json::Value =
            serde_json::from_slice(&resp.bytes).map_err(|_| Error::Auth(format!("OAuth token endpoint answered {} without JSON", resp.status)))?;
        Tokens::from_response(&v, old_refresh, mailcraft_core::message::time::now())
    }
}

impl OAuthBroker for HttpBroker {
    fn exchange(&self, provider: &Provider, client: &ClientCredentials, code: &str, verifier: &str, redirect: &str) -> Result<Tokens> {
        self.post(provider.token_url, exchange_body(client, code, verifier, redirect), "")
    }

    fn refresh(&self, provider: &Provider, client: &ClientCredentials, refresh_token: &str) -> Result<Tokens> {
        self.post(provider.token_url, refresh_body(client, refresh_token), refresh_token)
    }
}
