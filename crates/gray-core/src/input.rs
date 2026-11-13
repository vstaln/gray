use serde::{Deserialize, Serialize};
use std::fmt;

pub const STRUCTURED_INPUT_PROTOCOL: &str = "gray.input";

pub const LEGACY_STRUCTURED_INPUT_PROTOCOL: &str = "gray.discord.input";
pub const STRUCTURED_INPUT_VERSION: u32 = 1;
pub const MAX_INPUT_BYTES: usize = 1_048_576;
pub const MAX_KIND_CHARS: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputEnvelope {
    pub protocol: String,
    pub version: u32,
    pub kind: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone)]
pub enum InputError {
    TooLarge,
    Io,
    Json(String),
    UnsupportedProtocol,
    UnsupportedVersion,
    InvalidKind,
    InvalidPayload,
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => f.write_str("structured input exceeds the 1 MiB limit"),
            Self::Json(detail) => write!(f, "structured input is not valid JSON: {detail}"),
            Self::UnsupportedProtocol => f.write_str("unsupported structured input protocol"),
            Self::UnsupportedVersion => f.write_str("unsupported structured input version"),
            Self::InvalidKind => f.write_str("structured input kind is invalid"),
            Self::InvalidPayload => f.write_str("structured input payload must be an object"),
            Self::Io => f.write_str("could not read structured input"),
        }
    }
}

impl std::error::Error for InputError {}

impl InputEnvelope {
    pub fn from_json(bytes: &[u8]) -> Result<Self, InputError> {
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(InputError::TooLarge);
        }
        let envelope: Self =
            serde_json::from_slice(bytes).map_err(|error| InputError::Json(error.to_string()))?;
        envelope.validate()?;
        Ok(envelope)
    }

    pub fn validate(&self) -> Result<(), InputError> {
        if self.protocol != STRUCTURED_INPUT_PROTOCOL
            && self.protocol != LEGACY_STRUCTURED_INPUT_PROTOCOL
        {
            return Err(InputError::UnsupportedProtocol);
        }
        if self.version != STRUCTURED_INPUT_VERSION {
            return Err(InputError::UnsupportedVersion);
        }
        if self.kind.trim().is_empty() || self.kind.chars().count() > MAX_KIND_CHARS {
            return Err(InputError::InvalidKind);
        }
        if !self.payload.is_object() {
            return Err(InputError::InvalidPayload);
        }
        Ok(())
    }

    pub fn into_content_block(self) -> crate::message::ContentBlock {
        crate::message::ContentBlock::StructuredInput {
            protocol: self.protocol,
            version: self.version,
            kind: self.kind,
            payload: self.payload,
        }
    }
}
