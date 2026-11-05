//! The Microsoft account the launcher signed in, handed over in a session
//! file (docs/microsoft-account.md), and the Minecraft services call
//! online-mode servers need: authlib's MinecraftServicesSessionService.joinServer
//! through the services discovery document. The access token stays in this
//! process's memory: it is never logged, printed or written anywhere.
use std::{
    path::Path,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, SystemTime},
};

/// MinecraftServicesEnvironment.PROD.
pub const DEFAULT_DISCOVERY: &str = "https://discovery.minecraftservices.com/minecraft/client";
/// A session file older than this is refused: it can't be replayed later.
const MAX_SESSION_FILE_AGE: Duration = Duration::from_secs(120);

/// A signed-in account (vanilla's User with type msa).
pub struct AuthSession {
    pub name: String,
    pub uuid: u128,
    pub xuid: Option<String>,
    access_token: String,
    renewal: Option<Renewal>,
    expires_at: Option<u64>,
}

/// What the launcher hands over so the game can renew the account itself
/// when the access token runs out: the app's client ID and the refresh token.
#[derive(Clone)]
struct Renewal {
    client_id: String,
    refresh_token: String,
}
impl std::fmt::Debug for AuthSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthSession")
            .field("name", &self.name)
            .field("uuid", &horizon_protocol::crypto::uuid_string(self.uuid))
            .field("access_token", &"<hidden>")
            .finish()
    }
}

/// A UUID with or without dashes.
pub fn parse_uuid(text: &str) -> Option<u128> {
    let hex: String = text.chars().filter(|c| *c != '-').collect();
    (hex.len() == 32)
        .then(|| u128::from_str_radix(&hex, 16).ok())
        .flatten()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl AuthSession {
    /// Whether the game can renew this account by itself.
    pub fn can_renew(&self) -> bool {
        self.renewal.is_some()
    }
    /// The access token has run out, or does within `margin`.
    pub fn expires_within(&self, margin: Duration) -> bool {
        self.expires_at
            .is_some_and(|at| at <= unix_now() + margin.as_secs())
    }
    /// For tests and harnesses.
    pub fn new(name: &str, uuid: u128, access_token: &str) -> Self {
        Self {
            name: name.to_owned(),
            uuid,
            xuid: None,
            access_token: access_token.to_owned(),
            renewal: None,
            expires_at: None,
        }
    }
    /// The session file's JSON: {"name", "uuid", "access_token", "user_type": "msa", "xuid"}.
    pub fn parse(text: &str) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_str(text).map_err(|_| "the session is not valid JSON")?;
        let field = |k: &str| v.get(k).and_then(|s| s.as_str());
        let name = field("name").ok_or("the session has no name")?;
        if name.is_empty() || name.chars().count() > 16 {
            return Err("the session's name is not a valid player name".into());
        }
        let uuid = field("uuid")
            .and_then(parse_uuid)
            .ok_or("the session has no valid uuid")?;
        let access_token = field("access_token")
            .filter(|t| !t.is_empty())
            .ok_or("the session has no access token")?;
        if field("user_type").is_some_and(|t| t != "msa") {
            return Err("the session is not a Microsoft account session".into());
        }
        let renewal = match (field("client_id"), field("refresh_token")) {
            (Some(id), Some(token)) if !id.is_empty() && !token.is_empty() => Some(Renewal {
                client_id: id.to_owned(),
                refresh_token: token.to_owned(),
            }),
            _ => None,
        };
        Ok(Self {
            name: name.to_owned(),
            uuid,
            xuid: field("xuid").map(str::to_owned),
            access_token: access_token.to_owned(),
            renewal,
            expires_at: v.get("expires_at").and_then(|e| e.as_u64()),
        })
    }
    /// Read the session file the launcher wrote and delete it at once. It
    /// must be fresh and, on Unix, readable by its owner only.
    pub fn load(path: &Path) -> Result<Self, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let refuse = |why: &str| {
            let _ = std::fs::remove_file(path);
            Err(format!("{}: {why}", path.display()))
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                return refuse("other users can read it");
            }
        }
        let age = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .unwrap_or_default();
        if age > MAX_SESSION_FILE_AGE {
            return refuse("it is too old (start the game from the launcher)");
        }
        let text = std::fs::read_to_string(path);
        let _ = std::fs::remove_file(path);
        Self::parse(&text.map_err(|e| format!("{}: {e}", path.display()))?)
    }
}

static CURRENT: RwLock<Option<Arc<AuthSession>>> = RwLock::new(None);

/// The account this game was started with; a renewal replaces it.
pub fn set_current(session: AuthSession) {
    *CURRENT.write().unwrap() = Some(Arc::new(session));
}
pub fn current() -> Option<Arc<AuthSession>> {
    CURRENT.read().unwrap().clone()
}

/// Why joining a server through the session service failed, like the
/// AuthenticationException subclasses authlib throws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinError {
    /// No answer, or a 5xx (AuthenticationUnavailableException).
    Unavailable,
    /// 401 or ForbiddenOperationException (InvalidCredentialsException).
    InvalidSession,
    /// InsufficientPrivilegesException: multiplayer is off for the account.
    InsufficientPrivileges,
    /// multiplayer.access.banned or FORCED_USERNAME_CHANGE.
    Banned,
    /// Anything else, with the service's message.
    Other(String),
}
impl JoinError {
    /// MinecraftClientHttpException.toAuthenticationException.
    pub fn from_response(status: u16, body: &str) -> Self {
        let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
        let error = v.get("error").and_then(|e| e.as_str()).unwrap_or("");
        match error {
            "ForbiddenOperationException" => return Self::InvalidSession,
            "multiplayer.access.banned" | "FORCED_USERNAME_CHANGE" => return Self::Banned,
            "InsufficientPrivilegesException" => return Self::InsufficientPrivileges,
            _ => {}
        }
        match status {
            401 => Self::InvalidSession,
            500.. => Self::Unavailable,
            _ => Self::Other(
                v.get("errorMessage")
                    .and_then(|m| m.as_str())
                    .map_or_else(|| format!("Status: {status}"), str::to_owned),
            ),
        }
    }
    /// ClientHandshakePacketListenerImpl.authenticateServer's message: the
    /// key of the reason inside "disconnect.loginFailedInfo", or its text.
    pub fn reason(&self) -> Result<&'static str, &str> {
        match self {
            Self::Unavailable => Ok("disconnect.loginFailedInfo.serversUnavailable"),
            Self::InvalidSession => Ok("disconnect.loginFailedInfo.invalidSession"),
            Self::InsufficientPrivileges => Ok("disconnect.loginFailedInfo.insufficientPrivileges"),
            Self::Banned => Ok("disconnect.loginFailedInfo.userBanned"),
            Self::Other(message) => Err(message),
        }
    }
}

/// The services discovery document URL: `HORIZON_API_DISCOVERY` overrides
/// it like authlib's `-Dminecraft.api.discovery.host`.
pub fn discovery_url() -> String {
    std::env::var("HORIZON_API_DISCOVERY")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| DEFAULT_DISCOVERY.to_owned())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .user_agent(&format!("Horizon/{}", horizon_core::release::VERSION))
        .build()
}

/// The discovery document, fetched once per game (a failure is retried next time).
pub(crate) fn discovery() -> Result<Arc<serde_json::Value>, JoinError> {
    static CACHE: Mutex<Option<Arc<serde_json::Value>>> = Mutex::new(None);
    if let Some(d) = CACHE.lock().unwrap().clone() {
        return Ok(d);
    }
    let doc: serde_json::Value = agent()
        .get(&discovery_url())
        .call()
        .ok()
        .and_then(|r| r.into_string().ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .ok_or(JoinError::Unavailable)?;
    let doc = Arc::new(doc);
    *CACHE.lock().unwrap() = Some(doc.clone());
    Ok(doc)
}

/// MinecraftServicesDiscoveryService.getUrl(service, endpoint).
pub fn endpoint(doc: &serde_json::Value, service: &str, name: &str) -> Option<String> {
    doc.pointer(&format!("/discovery/{service}/endpoints/{name}/uri"))
        .and_then(|u| u.as_str())
        .map(str::to_owned)
}

/// The JoinMinecraftServerRequest body; `selectedProfile` is the undashed UUID.
pub fn join_request(session: &AuthSession, server_hash: &str) -> String {
    serde_json::json!({
        "accessToken": session.access_token,
        "selectedProfile": format!("{:032x}", session.uuid),
        "serverId": server_hash,
    })
    .to_string()
}

/// MinecraftServicesSessionService.joinServer: tell the session service this
/// account is joining the server whose hash is `server_hash`.
pub fn join_server(session: &AuthSession, server_hash: &str) -> Result<(), JoinError> {
    const FALLBACK: &str = "https://sessionserver.mojang.com/session/minecraft/join";
    let url = discovery()
        .ok()
        .and_then(|doc| endpoint(&doc, "session", "join"))
        .unwrap_or_else(|| FALLBACK.to_owned());
    let mut last = JoinError::Unavailable;
    const WAITS_MS: [u64; 6] = [0, 500, 1000, 2000, 3500, 5000];
    for (attempt, wait) in WAITS_MS.into_iter().enumerate() {
        if wait > 0 {
            std::thread::sleep(Duration::from_millis(wait));
        }
        let result = agent()
            .post(&url)
            .set("Content-Type", "application/json; charset=utf-8")
            .send_string(&join_request(session, server_hash));
        match result {
            Ok(_) => return Ok(()),
            Err(ureq::Error::Status(status, response)) => {
                let error = JoinError::from_response(
                    status,
                    &response.into_string().unwrap_or_default(),
                );
                if status != 429 && error != JoinError::Unavailable {
                    return Err(error);
                }
                log::warn!("session join attempt {} answered {status}", attempt + 1);
                last = JoinError::Unavailable;
            }
            Err(ureq::Error::Transport(e)) => {
                log::warn!("session join attempt {} failed: {e}", attempt + 1);
                last = JoinError::Unavailable;
            }
        }
    }
    Err(last)
}

const MS_TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

fn post_json(url: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
    agent()
        .post(url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| match e {
            ureq::Error::Status(status, _) => format!("answered {status}"),
            ureq::Error::Transport(t) => t.to_string(),
        })?
        .into_string()
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
}

/// Renew the signed-in account without the launcher: the refresh token gives
/// a Microsoft token, that an Xbox and then a Minecraft token (the same steps
/// as the launcher). On success the new session replaces the current one.
/// Only one renewal runs at a time; a caller that waited gets its result.
pub fn renew() -> Result<Arc<AuthSession>, String> {
    static BUSY: Mutex<()> = Mutex::new(());
    let before = current().ok_or("not signed in")?;
    let _guard = BUSY.lock().unwrap();
    let now = current().ok_or("not signed in")?;
    if !Arc::ptr_eq(&before, &now) {
        return Ok(now);
    }
    let renewal = now
        .renewal
        .clone()
        .ok_or("the launcher gave no way to renew the sign-in")?;
    let tokens: serde_json::Value = agent()
        .post(MS_TOKEN_URL)
        .send_form(&[
            ("grant_type", "refresh_token"),
            ("client_id", &renewal.client_id),
            ("refresh_token", &renewal.refresh_token),
            ("scope", "XboxLive.signin offline_access"),
        ])
        .map_err(|e| match e {
            ureq::Error::Status(400 | 401, _) => {
                "the saved sign-in is no longer valid, sign in again in the launcher".to_owned()
            }
            ureq::Error::Status(status, _) => format!("Microsoft answered {status}"),
            ureq::Error::Transport(t) => t.to_string(),
        })?
        .into_string()
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))?;
    let ms_access = tokens["access_token"]
        .as_str()
        .ok_or("Microsoft's reply has no access token")?;
    let refresh_token = tokens["refresh_token"]
        .as_str()
        .unwrap_or(&renewal.refresh_token)
        .to_owned();
    let xbl = post_json(
        XBL_URL,
        serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={ms_access}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }),
    )
    .map_err(|e| format!("Xbox Live: {e}"))?;
    let xbl_token = xbl["Token"].as_str().ok_or("Xbox reply has no token")?;
    let xsts = post_json(
        XSTS_URL,
        serde_json::json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        }),
    )
    .map_err(|e| format!("Xbox Live: {e}"))?;
    let xsts_token = xsts["Token"].as_str().ok_or("XSTS reply has no token")?;
    let uhs = xsts["DisplayClaims"]["xui"][0]["uhs"]
        .as_str()
        .ok_or("XSTS reply has no user hash")?;
    let login = post_json(
        MC_LOGIN_URL,
        serde_json::json!({ "identityToken": format!("XBL3.0 x={uhs};{xsts_token}") }),
    )
    .map_err(|e| format!("Minecraft login: {e}"))?;
    let access_token = login["access_token"]
        .as_str()
        .ok_or("Minecraft's reply has no access token")?
        .to_owned();
    let profile: serde_json::Value = agent()
        .get(MC_PROFILE_URL)
        .set("Authorization", &format!("Bearer {access_token}"))
        .call()
        .map_err(|e| format!("Minecraft profile: {e}"))?
        .into_string()
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))?;
    if profile["id"].as_str().and_then(parse_uuid) != Some(now.uuid) {
        return Err("the renewed sign-in belongs to a different account".into());
    }
    let lifetime = login["expires_in"].as_u64().unwrap_or(3600);
    let renewed = Arc::new(AuthSession {
        name: now.name.clone(),
        uuid: now.uuid,
        xuid: now.xuid.clone(),
        access_token,
        renewal: Some(Renewal {
            client_id: renewal.client_id,
            refresh_token,
        }),
        expires_at: Some(unix_now() + lifetime),
    });
    *CURRENT.write().unwrap() = Some(renewed.clone());
    log::info!("renewed the sign-in of {}", renewed.name);
    Ok(renewed)
}

/// What the session service says about a player joining this server
/// (MinecraftServicesSessionService.hasJoinedServer).
#[derive(Clone, Debug, PartialEq)]
pub struct JoinedProfile {
    pub uuid: u128,
    pub properties: Vec<horizon_protocol::packets::login::Property>,
}

/// HttpDiscoveryService.buildQuery: `URLEncoder.encode(value, UTF-8)`.
fn url_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'-' | b'*' | b'_' => {
                out.push(char::from(b));
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The session service's verify endpoint, asked by an online-mode server
/// whether `name` joined with `server_hash` (and from `ip`, when the server
/// prevents proxy connections). `Ok(None)`: it didn't, or the service said
/// no; `Err(Unavailable)`: no answer.
pub fn has_joined(
    name: &str,
    server_hash: &str,
    ip: Option<std::net::IpAddr>,
) -> Result<Option<JoinedProfile>, JoinError> {
    let doc = discovery()?;
    let url = endpoint(&doc, "session", "verify").ok_or(JoinError::Unavailable)?;
    let mut query = format!(
        "username={}&serverId={}",
        url_encode(name),
        url_encode(server_hash)
    );
    if let Some(ip) = ip {
        query.push_str(&format!("&ip={}", url_encode(&ip.to_string())));
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    let body = match agent().get(&format!("{url}{separator}{query}")).call() {
        Ok(response) => response.into_string().unwrap_or_default(),
        Err(ureq::Error::Status(status, response)) => {
            let error =
                JoinError::from_response(status, &response.into_string().unwrap_or_default());
            return match error {
                JoinError::Unavailable => Err(error),
                _ => Ok(None),
            };
        }
        Err(ureq::Error::Transport(_)) => return Err(JoinError::Unavailable),
    };
    Ok(parse_joined(&body))
}

/// HasJoinedMinecraftServerResponse: `id` and `properties`; no id is no profile.
pub fn parse_joined(body: &str) -> Option<JoinedProfile> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let uuid = parse_uuid(v.get("id")?.as_str()?)?;
    let properties = v
        .get("properties")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some(horizon_protocol::packets::login::Property {
                name: p.get("name")?.as_str()?.to_owned(),
                value: p.get("value")?.as_str()?.to_owned(),
                signature: p
                    .get("signature")
                    .and_then(|s| s.as_str())
                    .map(str::to_owned),
            })
        })
        .collect();
    Some(JoinedProfile { uuid, properties })
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: u128 = 0x069a79f444e94726a5befca90e38aaf5;

    fn write(dir: &Path, text: &str, mode: u32) -> std::path::PathBuf {
        let path = dir.join("session.json");
        std::fs::write(&path, text).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let _ = mode;
        path
    }

    #[test]
    fn verify_answers() {
        assert_eq!(url_encode("Tester_1"), "Tester_1");
        assert_eq!(url_encode("-5a f"), "-5a+f");
        assert_eq!(url_encode("::1"), "%3A%3A1");
        let body = r#"{"id":"069a79f444e94726a5befca90e38aaf5","name":"Notch","properties":[{"name":"textures","value":"e30=","signature":"c2ln"}]}"#;
        let p = parse_joined(body).unwrap();
        assert_eq!(p.uuid, UUID);
        assert_eq!(p.properties.len(), 1);
        assert_eq!(p.properties[0].signature.as_deref(), Some("c2ln"));
        assert_eq!(parse_joined(""), None);
        assert_eq!(parse_joined(r#"{"name":"Notch"}"#), None);
    }

    #[test]
    fn session_files_are_read_once_and_must_be_private() {
        let dir = std::env::temp_dir().join(format!("horizon-auth-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = r#"{"name":"Notch","uuid":"069a79f4-44e9-4726-a5be-fca90e38aaf5","access_token":"mock-token","user_type":"msa"}"#;
        let path = write(&dir, text, 0o600);
        let s = AuthSession::load(&path).unwrap();
        assert_eq!((s.name.as_str(), s.uuid), ("Notch", UUID));
        assert!(!path.exists(), "deleted after reading");
        assert!(!format!("{s:?}").contains("mock-token"));
        #[cfg(unix)]
        {
            let path = write(&dir, text, 0o644);
            assert!(
                AuthSession::load(&path)
                    .unwrap_err()
                    .contains("other users")
            );
            assert!(!path.exists());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_can_carry_what_the_game_needs_to_renew_it() {
        let plain = AuthSession::parse(
            r#"{"name":"A","uuid":"069a79f444e94726a5befca90e38aaf5","access_token":"t"}"#,
        )
        .unwrap();
        assert!(!plain.can_renew());
        assert!(!plain.expires_within(Duration::from_secs(300)));
        let full = AuthSession::parse(
            r#"{"name":"A","uuid":"069a79f444e94726a5befca90e38aaf5","access_token":"t","client_id":"c","refresh_token":"r","expires_at":1}"#,
        )
        .unwrap();
        assert!(full.can_renew());
        assert!(full.expires_within(Duration::from_secs(300)));
        let later = unix_now() + 7200;
        let fresh = AuthSession::parse(&format!(
            r#"{{"name":"A","uuid":"069a79f444e94726a5befca90e38aaf5","access_token":"t","expires_at":{later}}}"#
        ))
        .unwrap();
        assert!(!fresh.expires_within(Duration::from_secs(300)));
        assert!(fresh.expires_within(Duration::from_secs(8000)));
    }

    #[test]
    fn session_file_fields() {
        assert!(
            AuthSession::parse(r#"{"name":"Notch","uuid":"069a79f444e94726a5befca90e38aaf5"}"#)
                .is_err()
        );
        assert!(
            AuthSession::parse(
                r#"{"name":"","uuid":"069a79f444e94726a5befca90e38aaf5","access_token":"t"}"#
            )
            .is_err()
        );
        assert!(AuthSession::parse(r#"{"name":"A","uuid":"nope","access_token":"t"}"#).is_err());
        assert!(AuthSession::parse(r#"{"name":"A","uuid":"069a79f444e94726a5befca90e38aaf5","access_token":"t","user_type":"legacy"}"#).is_err());
        let s = AuthSession::parse(r#"{"name":"A","uuid":"069a79f444e94726a5befca90e38aaf5","access_token":"t","xuid":"25"}"#).unwrap();
        assert_eq!(s.xuid.as_deref(), Some("25"));
    }

    #[test]
    fn join_request_like_authlib() {
        let s = AuthSession::new("Notch", UUID, "tok");
        let v: serde_json::Value = serde_json::from_str(&join_request(&s, "-7c9d")).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"accessToken": "tok", "selectedProfile": "069a79f444e94726a5befca90e38aaf5", "serverId": "-7c9d"})
        );
    }

    #[test]
    fn errors_map_like_authlib() {
        let e = |status, body| JoinError::from_response(status, body);
        assert_eq!(
            e(
                403,
                r#"{"error":"ForbiddenOperationException","errorMessage":"Invalid token"}"#
            ),
            JoinError::InvalidSession
        );
        assert_eq!(e(401, ""), JoinError::InvalidSession);
        assert_eq!(
            e(403, r#"{"error":"InsufficientPrivilegesException"}"#),
            JoinError::InsufficientPrivileges
        );
        assert_eq!(
            e(403, r#"{"error":"multiplayer.access.banned"}"#),
            JoinError::Banned
        );
        assert_eq!(e(503, "down"), JoinError::Unavailable);
        assert_eq!(
            e(429, r#"{"errorMessage":"Too many"}"#),
            JoinError::Other("Too many".into())
        );
        assert_eq!(
            JoinError::InvalidSession.reason(),
            Ok("disconnect.loginFailedInfo.invalidSession")
        );
    }

    #[test]
    fn discovery_endpoints() {
        let doc = serde_json::json!({"discovery": {"session": {"endpoints": {"join": {"uri": "https://s/join"}}}}});
        assert_eq!(
            endpoint(&doc, "session", "join").as_deref(),
            Some("https://s/join")
        );
        assert_eq!(endpoint(&doc, "session", "verify"), None);
    }
}
