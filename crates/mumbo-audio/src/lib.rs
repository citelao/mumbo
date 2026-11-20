pub mod mic;
pub mod speaker;
#[cfg(test)]
mod test_util;

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("Audio processing error: {0}")]
    Sonora(#[from] sonora::Error),

    #[error("Opus error: {0}")]
    Opus(#[from] opus::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
