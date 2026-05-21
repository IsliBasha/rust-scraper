use thiserror::Error;

#[derive(Debug, Error, Clone)]
pub enum CrawlError {
    #[error("HTTP error: status {status}")]
    HttpStatus { status: u16 },

    #[error("Network error: {message}")]
    Network { message: String },

    #[error("Timeout after {elapsed_ms}ms")]
    Timeout { elapsed_ms: u64 },

    #[error("Too many redirects")]
    TooManyRedirects,

    #[error("Browser error: {message}")]
    Browser { message: String },

    #[error("Parse error: {message}")]
    Parse { message: String },

    #[error("Extraction error: {message}")]
    Extraction { message: String },

    #[error("Storage error: {message}")]
    Storage { message: String },

    #[error("Serialization error: {message}")]
    Serialization { message: String },

    #[error("Invalid URL: {message}")]
    InvalidUrl { message: String },

    #[error("Configuration error: {message}")]
    Config { message: String },

    #[error("Shutdown requested")]
    Shutdown,

    #[error("Rate limit exceeded for host: {host}")]
    RateLimited { host: String },
}

impl CrawlError {
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Network { .. } | Self::Timeout { .. } | Self::RateLimited { .. } => true,
            Self::HttpStatus { status } => *status == 429 || *status >= 500,
            _ => false,
        }
    }

    pub fn network(msg: impl Into<String>) -> Self {
        Self::Network {
            message: msg.into(),
        }
    }

    pub fn browser(msg: impl Into<String>) -> Self {
        Self::Browser {
            message: msg.into(),
        }
    }

    pub fn storage(msg: impl Into<String>) -> Self {
        Self::Storage {
            message: msg.into(),
        }
    }

    pub fn parse(msg: impl Into<String>) -> Self {
        Self::Parse {
            message: msg.into(),
        }
    }

    pub fn extraction(msg: impl Into<String>) -> Self {
        Self::Extraction {
            message: msg.into(),
        }
    }

    pub fn invalid_url(msg: impl Into<String>) -> Self {
        Self::InvalidUrl {
            message: msg.into(),
        }
    }

    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config {
            message: msg.into(),
        }
    }
}
