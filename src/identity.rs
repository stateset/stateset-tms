//! Clerk staff identity. Only signed short-lived v2 session tokens are accepted;
//! tenant grants are deployment-owned and roles/membership/session status are
//! read from Clerk on each request, never from browser metadata or JWT roles.
use crate::auth::{Principal, Role};
use chrono::Utc;
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationGrant {
    pub tenant_id: String,
    pub roles: BTreeMap<String, Role>,
    #[serde(default)]
    pub counterparty_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClerkConfig {
    pub issuer: String,
    pub secret_key: String,
    pub authorized_parties: Vec<String>,
    pub organizations: BTreeMap<String, OrganizationGrant>,
    #[serde(default)]
    pub audience: Option<String>,
    // Only a loopback fixture in explicitly enabled demo mode may override BAPI.
    #[serde(default)]
    pub fixture_api_url: Option<String>,
}
#[derive(Debug)]
pub enum IdentityError {
    Unauthenticated,
    Forbidden,
    Unavailable,
}
#[derive(Clone)]
pub struct ClerkIdentity {
    config: Arc<ClerkConfig>,
    client: Client,
    api: String,
    keys: Arc<Mutex<Option<(Instant, JwkSet)>>>,
}
pub struct StaffIdentity {
    pub principal: Principal,
    pub organization_id: String,
}
#[derive(Clone, Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    sid: String,
    azp: String,
    exp: i64,
    iat: i64,
    nbf: i64,
    v: u8,
    #[serde(default)]
    o: Option<OrganizationClaim>,
    #[serde(default)]
    sts: Option<String>,
    #[serde(default)]
    act: Option<Value>,
    #[serde(default)]
    aud: Option<Value>,
}
#[derive(Clone, Deserialize)]
struct OrganizationClaim {
    id: String,
}
fn id(value: &str, prefix: &str, max: usize) -> bool {
    value.starts_with(prefix)
        && value.len() > prefix.len()
        && value.len() <= max
        && value
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || b"_-".contains(&v))
}
fn origin(value: &str, loopback: bool) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path() == "/"
        && url.origin().ascii_serialization() == value
        && (url.scheme() == "https"
            || (loopback
                && url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))))
}
impl ClerkIdentity {
    pub fn from_env(demo: bool) -> Result<Option<Self>, String> {
        let raw = std::env::var("FREIGHT_CLERK_JSON")
            .ok()
            .filter(|v| !v.is_empty());
        raw.map(|value| {
            let config: ClerkConfig =
                serde_json::from_str(&value).map_err(|_| "Invalid FREIGHT_CLERK_JSON fields")?;
            Self::new(config, demo)
        })
        .transpose()
    }
    pub fn new(config: ClerkConfig, demo: bool) -> Result<Self, String> {
        if !origin(&config.issuer, demo)
            || !(32..=512).contains(&config.secret_key.len())
            || !config.secret_key.bytes().all(|v| v.is_ascii_graphic())
            || config.authorized_parties.is_empty()
            || config.authorized_parties.len() > 20
            || config.authorized_parties.iter().any(|v| !origin(v, demo))
            || config.organizations.is_empty()
            || config.organizations.len() > 1000
            || config
                .audience
                .as_ref()
                .is_some_and(|v| v.trim().is_empty() || v.len() > 200)
        {
            return Err("Clerk requires a valid HTTPS issuer/origins, secret, explicit organizations, and valid audience; HTTP loopback is demo-only".into());
        }
        for (organization, grant) in &config.organizations {
            if !id(organization, "org_", 128)
                || grant.tenant_id.is_empty()
                || grant.tenant_id.len() > 100
                || !grant
                    .tenant_id
                    .bytes()
                    .all(|v| v.is_ascii_alphanumeric() || b"-_.".contains(&v))
                || grant.roles.values().any(|role| {
                    !Principal {
                        tenant_id: grant.tenant_id.clone(),
                        subject: "configuration".into(),
                        role: *role,
                        counterparty_id: grant.counterparty_id.clone(),
                    }
                    .valid_scope()
                })
                || (grant.counterparty_id.is_some()
                    && grant
                        .roles
                        .values()
                        .any(|v| Some(v) != grant.roles.values().next()))
                || grant.roles.is_empty()
                || grant.roles.len() > 50
                || grant.roles.keys().any(|v| {
                    !v.starts_with("org:")
                        || v.len() > 100
                        || !v
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b":_-".contains(&c))
                })
            {
                return Err("Invalid Clerk organization, tenant, or explicit role mapping".into());
            }
        }
        let api = if let Some(value) = &config.fixture_api_url {
            if !demo || !value.starts_with("http://") || !origin(value, true) {
                return Err("Clerk API override requires a demo-only HTTP loopback fixture".into());
            }
            value.clone()
        } else {
            "https://api.clerk.com/v1".into()
        };
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Cannot initialize Clerk HTTP client")?;
        Ok(Self {
            config: Arc::new(config),
            client,
            api,
            keys: Arc::new(Mutex::new(None)),
        })
    }
    async fn json(
        &self,
        url: &str,
        parameters: &[(&str, &str)],
        private: bool,
    ) -> Result<Value, IdentityError> {
        let mut request = self.client.get(url).query(parameters);
        if private {
            request = request.bearer_auth(&self.config.secret_key);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| IdentityError::Unavailable)?;
        if response.status().as_u16() == 404 {
            return Err(IdentityError::Unauthenticated);
        }
        if !response.status().is_success() || response.content_length().is_some_and(|n| n > 131072)
        {
            return Err(IdentityError::Unavailable);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| IdentityError::Unavailable)?
        {
            if bytes.len() + chunk.len() > 131072 {
                return Err(IdentityError::Unavailable);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| IdentityError::Unavailable)
    }
    async fn key(&self, kid: &str) -> Result<DecodingKey, IdentityError> {
        let mut cached = self.keys.lock().await;
        let refresh = match &*cached {
            None => true,
            Some((at, keys)) => {
                at.elapsed() > Duration::from_secs(300)
                    || (keys.find(kid).is_none() && at.elapsed() > Duration::from_secs(1))
            }
        };
        if refresh {
            let json = self
                .json(
                    &format!("{}/.well-known/jwks.json", self.config.issuer),
                    &[],
                    false,
                )
                .await?;
            let keys: JwkSet =
                serde_json::from_value(json).map_err(|_| IdentityError::Unavailable)?;
            if keys.keys.is_empty() || keys.keys.len() > 20 {
                return Err(IdentityError::Unavailable);
            }
            *cached = Some((Instant::now(), keys));
        }
        let jwk = cached
            .as_ref()
            .and_then(|(_, keys)| keys.find(kid))
            .ok_or(IdentityError::Unauthenticated)?;
        // Never accept symmetric keys or algorithms selected by an untrusted token.
        let raw = serde_json::to_value(jwk).map_err(|_| IdentityError::Unavailable)?;
        if raw["kty"] != "RSA"
            || raw["alg"].as_str().is_some_and(|v| v != "RS256")
            || raw["use"].as_str().is_some_and(|v| v != "sig")
        {
            return Err(IdentityError::Unauthenticated);
        }
        DecodingKey::from_jwk(jwk).map_err(|_| IdentityError::Unauthenticated)
    }
    pub async fn verify(&self, token: &str) -> Result<StaffIdentity, IdentityError> {
        if token.len() > 16384 {
            return Err(IdentityError::Unauthenticated);
        }
        let header = decode_header(token).map_err(|_| IdentityError::Unauthenticated)?;
        if header.alg != Algorithm::RS256 {
            return Err(IdentityError::Unauthenticated);
        }
        let kid = header
            .kid
            .filter(|v| {
                !v.is_empty()
                    && v.len() <= 128
                    && v.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            })
            .ok_or(IdentityError::Unauthenticated)?;
        let key = self.key(&kid).await?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.leeway = 5;
        validation.validate_nbf = true;
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_required_spec_claims(&["iss", "sub", "exp", "nbf"]);
        if let Some(aud) = &self.config.audience {
            validation.set_audience(&[aud]);
            validation.required_spec_claims.insert("aud".into());
        } else {
            validation.validate_aud = false;
        }
        let claims = decode::<Claims>(token, &key, &validation)
            .map_err(|_| IdentityError::Unauthenticated)?
            .claims;
        let now = Utc::now().timestamp();
        if claims.iss != self.config.issuer
            || claims.v != 2
            || !id(&claims.sub, "user_", 94)
            || !id(&claims.sid, "sess_", 128)
            || !self.config.authorized_parties.contains(&claims.azp)
            || claims.exp <= now
            || claims.iat > now + 5
            || claims
                .exp
                .checked_sub(claims.iat)
                .is_none_or(|age| !(1..=120).contains(&age))
            || claims.nbf > claims.exp
            || claims.nbf > now + 5
            || claims.act.is_some()
            || claims.sts.as_deref().is_some_and(|v| v != "active")
            || (self.config.audience.is_none() && claims.aud.is_some())
        {
            return Err(IdentityError::Unauthenticated);
        }
        let organization = claims
            .o
            .map(|v| v.id)
            .filter(|v| id(v, "org_", 128))
            .ok_or(IdentityError::Forbidden)?;
        let grant = self
            .config
            .organizations
            .get(&organization)
            .ok_or(IdentityError::Forbidden)?;
        let session_url = format!("{}/sessions/{}", self.api, claims.sid);
        let membership_url = format!("{}/organizations/{}/memberships", self.api, organization);
        let parameters = [("user_id", claims.sub.as_str()), ("limit", "2")];
        let (session, memberships) = tokio::try_join!(
            self.json(&session_url, &[], true),
            self.json(&membership_url, &parameters, true)
        )?;
        if session["id"] != claims.sid
            || session["user_id"] != claims.sub
            || session["status"] != "active"
            || session.get("actor").is_some_and(|v| !v.is_null())
            || session["expire_at"]
                .as_i64()
                .is_none_or(|v| v <= Utc::now().timestamp_millis())
        {
            return Err(IdentityError::Unauthenticated);
        }
        if session["last_active_organization_id"]
            .as_str()
            .is_some_and(|v| v != organization)
        {
            return Err(IdentityError::Forbidden);
        }
        let members = memberships["data"]
            .as_array()
            .ok_or(IdentityError::Unavailable)?;
        if members.len() != 1 || memberships["total_count"].as_u64() != Some(1) {
            return Err(IdentityError::Forbidden);
        }
        let membership = &members[0];
        if membership["organization"]["id"] != organization
            || membership["public_user_data"]["user_id"] != claims.sub
        {
            return Err(IdentityError::Forbidden);
        }
        let role = membership["role"]
            .as_str()
            .and_then(|v| grant.roles.get(v))
            .copied()
            .ok_or(IdentityError::Forbidden)?;
        Ok(StaffIdentity {
            principal: Principal {
                tenant_id: grant.tenant_id.clone(),
                subject: format!("clerk:{}", claims.sub),
                role,
                counterparty_id: grant.counterparty_id.clone(),
            },
            organization_id: organization,
        })
    }
}
