pub mod cli;
pub mod client;
pub mod generator;
pub mod models;
pub mod render;
pub mod selection;
pub mod tui;

use std::error::Error;
use std::fmt;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug)]
pub struct AppError(pub String);

impl AppError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(error: reqwest::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(error: serde_json::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<url::ParseError> for AppError {
    fn from(error: url::ParseError) -> Self {
        Self(error.to_string())
    }
}
