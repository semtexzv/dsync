use std::future::{Ready, ready};
use std::net::SocketAddr;
use std::ops::Add;
use std::time::{Duration, SystemTime};
use anyhow::anyhow;
use hyper::body::Incoming;
use hyper::Response;
use hyper::server::conn::http1;
use hyper::service::Service;
use oauth2::{AccessToken, AuthorizationCode, AuthUrl, ClientId, ClientSecret, CsrfToken, ExtraTokenFields, PkceCodeChallenge, RedirectUrl, RefreshToken, Scope, StandardRevocableToken, StandardTokenResponse, TokenResponse, TokenUrl};
use oauth2::basic::{BasicClient, BasicTokenResponse};
use reqwest::Client;
use serde::Deserialize;
use tokio::net::TcpListener;
use tracing::{debug, info};

const CLIENT_ID: &str = env!("GOOGLE_CLIENT_ID");
const CLIENT_SECRET: &str = env!("GOOGLE_CLIENT_SECRET");
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://www.googleapis.com/oauth2/v3/token";

async fn auth_server(list: TcpListener) -> anyhow::Result<AuthorizationCode> {
    #[derive(Debug, Deserialize)]
    struct RedirectCallbackQuery {
        state: String,
        code: AuthorizationCode,
    }

    struct OauthCallbackService {
        tx: tokio::sync::mpsc::Sender<AuthorizationCode>,
    }

    impl Service<hyper::Request<Incoming>> for OauthCallbackService {
        type Response = hyper::Response<String>;
        type Error = anyhow::Error;
        type Future = Ready<Result<Self::Response, Self::Error>>;

        fn call(&self, req: hyper::Request<Incoming>) -> Self::Future {
            let query = if let Some(query) = req.uri().query() {
                info!("Query: {query:?}");
                query
            } else {
                return ready(Err(anyhow!("Invalid query")));
            };

            if let Ok(query) = serde_urlencoded::from_str::<RedirectCallbackQuery>(query) {
                self.tx.try_send(query.code).unwrap();
                return ready(Ok(Response::builder().body("Sucesfully logged in".to_string()).unwrap()));
            }
            return ready(Err(anyhow!("Invalid query")));
        }
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    loop {
        let tx = tx.clone();
        let (stream, _) = list.accept().await.unwrap();
        let stream = hyper_util::rt::TokioIo::new(stream);

        let serve = http1::Builder::new()
            .serve_connection(stream, OauthCallbackService { tx });

        tokio::select! {
            done = serve => {
                panic!("Server closed");
            }
            code = rx.recv() => {
                return Ok(code.unwrap())
            }
        }
    }
}

pub(crate) async fn auth(client: &Client) -> anyhow::Result<(SystemTime, BasicTokenResponse)> {
    let port: u16 = 33344;
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let list = TcpListener::bind(addr).await?;


    let client_id = ClientId::new(CLIENT_ID.into());
    let client_secret = ClientSecret::new(CLIENT_SECRET.into());
    let auth_url = AuthUrl::new(AUTH_URL.into()).unwrap();
    let token_url = TokenUrl::new(TOKEN_URL.into()).unwrap();

    let device_client = BasicClient::new(client_id)
        .set_client_secret(client_secret)
        .set_auth_uri(auth_url)
        .set_token_uri(token_url)
        .set_redirect_uri(
            RedirectUrl::new(format!("http://localhost:{port}")).expect("Invalid redirect URL"),
        );

    let (pkce_code_challenge, pkce_code_verifier) = PkceCodeChallenge::new_random_sha256();

    let (authorize_url, csrf_state) = device_client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new(
            "https://www.googleapis.com/auth/drive".to_string(),
        ))
        .set_pkce_challenge(pkce_code_challenge)
        .url();

    open::that(&authorize_url.to_string()).unwrap();

    let code = auth_server(list).await.unwrap();

    let token_response = device_client
        .exchange_code(code)
        .set_pkce_verifier(pkce_code_verifier)
        .request_async(client)
        .await?;

    let time = SystemTime::now().add(token_response.expires_in().unwrap());
    info!("Received token: {token_response:?}, valid until {time:?}");

    Ok((time, token_response))
}

pub async fn refresh(client: &Client, refresh_token: &RefreshToken) -> anyhow::Result<(SystemTime, BasicTokenResponse)> {
    let client_id = ClientId::new(CLIENT_ID.into());
    let client_secret = ClientSecret::new(CLIENT_SECRET.into());
    let auth_url = AuthUrl::new(AUTH_URL.into()).unwrap();
    let token_url = TokenUrl::new(TOKEN_URL.into()).unwrap();

    let oauth_client = BasicClient::new(client_id)
        .set_client_secret(client_secret)
        .set_auth_uri(auth_url)
        .set_token_uri(token_url);

    let token_response = oauth_client.exchange_refresh_token(refresh_token)
        .request_async(client)
        .await?;

    let time = SystemTime::now().add(token_response.expires_in().unwrap());
    debug!("Refreshed token: {token_response:?}, valid until {time:?}");

    Ok((time, token_response))
}
