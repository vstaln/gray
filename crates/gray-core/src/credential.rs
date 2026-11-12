use std::collections::BTreeMap;
use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroizing;

pub const MAX_CREDENTIAL_BYTES: usize = 128 * 1024;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

impl Serialize for SecretString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(SecretString::new)
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretMap(BTreeMap<String, SecretString>);

impl SecretMap {
    #[allow(clippy::should_implement_trait)]
    pub fn from_iter<I, K, V>(iter: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self(
            iter.into_iter()
                .map(|(key, value)| (key.into(), SecretString::new(value)))
                .collect(),
        )
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(SecretString::as_str)
    }

    pub fn insert(
        &mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Option<SecretString> {
        self.0.insert(key.into(), SecretString::new(value))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }
}

impl fmt::Debug for SecretMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretMap(<redacted>, {} fields)", self.len())
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialMaterial {
    pub secrets: SecretMap,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    #[serde(default)]
    pub expires_at: Option<u64>,
}

impl CredentialMaterial {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn validate(&self) -> Result<(), CredentialError> {
        let encoded = serde_json::to_vec(self)
            .map_err(|_| CredentialError::Invalid("credential serialization failed".into()))?;
        if encoded.len() > MAX_CREDENTIAL_BYTES {
            return Err(CredentialError::Invalid(
                "credential payload too large".into(),
            ));
        }
        Ok(())
    }
}

impl fmt::Debug for CredentialMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialMaterial")
            .field("secrets", &self.secrets)
            .field("metadata_fields", &self.metadata.len())
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialEnvelope {
    pub version: u8,
    pub plugin: String,
    pub provider: String,
    pub auth_method: String,
    pub profile_binding: String,
    pub credential: CredentialMaterial,
}

impl CredentialEnvelope {
    pub fn new(
        plugin: impl Into<String>,
        provider: impl Into<String>,
        auth_method: impl Into<String>,
        profile_binding: impl Into<String>,
        credential: CredentialMaterial,
    ) -> Result<Self, CredentialError> {
        let plugin = plugin.into();
        let provider = provider.into();
        let auth_method = auth_method.into();
        let profile_binding = profile_binding.into();
        for (value, label) in [
            (&plugin, "plugin"),
            (&provider, "provider"),
            (&auth_method, "auth method"),
            (&profile_binding, "profile binding"),
        ] {
            if value.trim().is_empty() || value.contains(['\r', '\n', '\0']) {
                return Err(CredentialError::Invalid(format!(
                    "invalid credential {label}"
                )));
            }
        }
        credential.validate()?;
        Ok(Self {
            version: 1,
            plugin,
            provider,
            auth_method,
            profile_binding,
            credential,
        })
    }
}

impl fmt::Debug for CredentialEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialEnvelope")
            .field("version", &self.version)
            .field("plugin", &self.plugin)
            .field("provider", &self.provider)
            .field("auth_method", &self.auth_method)
            .field("profile_binding", &self.profile_binding)
            .field("credential", &self.credential)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct CredentialLease {
    pub secrets: SecretMap,
    pub metadata: BTreeMap<String, String>,
}

impl fmt::Debug for CredentialLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialLease")
            .field("secrets", &self.secrets)
            .field("metadata_fields", &self.metadata.len())
            .finish()
    }
}

#[derive(thiserror::Error)]
pub enum CredentialError {
    #[error("provider login required: {0}")]
    ReauthRequired(String),
    #[error("credential provider unavailable: {0}")]
    Unavailable(String),
    #[error("credential store rejected: {0}")]
    Store(String),
    #[error("credential rejected: {0}")]
    Invalid(String),
}

impl fmt::Debug for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReauthRequired(_) => f.write_str("ReauthRequired(<redacted>)"),
            Self::Unavailable(_) => f.write_str("Unavailable(<redacted>)"),
            Self::Store(_) => f.write_str("Store(<redacted>)"),
            Self::Invalid(_) => f.write_str("Invalid(<redacted>)"),
        }
    }
}

#[async_trait]
pub trait CredentialSource: Send + Sync {
    async fn acquire(&self) -> Result<CredentialLease, CredentialError>;
}
