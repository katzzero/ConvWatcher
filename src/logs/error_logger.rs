use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::config::global::GlobalConfig;

/// Secure file permissions: 600 (owner read/write only)
#[cfg(unix)]
const SECURE_FILE_MODE: u32 = 0o600;

/// Secure directory permissions: 700 (owner read/write/execute only)
#[cfg(unix)]
const SECURE_DIR_MODE: u32 = 0o700;

pub struct ErrorLogger {
    file: Mutex<PathBuf>,
    max_size_mb: u64,
    redact_secrets: bool,
    embedded_secret: String,
}

impl ErrorLogger {
    pub fn new(global_config: &GlobalConfig) -> Result<Self, std::io::Error> {
        let path = PathBuf::from(&global_config.log.errors_file);
        if let Some(parent) = path.parent() {
            #[cfg(unix)]
            {
                fs::create_dir_all(parent)?;
                // Set secure permissions on the log directory
                let perms = fs::Permissions::from_mode(SECURE_DIR_MODE);
                let _ = fs::set_permissions(parent, perms);
            }
            #[cfg(not(unix))]
            {
                fs::create_dir_all(parent)?;
            }
        }
        Ok(Self {
            file: Mutex::new(path),
            max_size_mb: global_config.log.max_error_log_size_mb,
            redact_secrets: global_config.log.redact_secrets,
            embedded_secret: global_config.embedded_secret.clone(),
        })
    }

    pub fn log(&self, message: &str, file_name: &str, context: &str) {
        let path = self.file.lock().unwrap_or_else(|p| p.into_inner());
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");

        // Sanitize message and file_name if redaction is enabled
        let sanitized_message = if self.redact_secrets {
            let mut secrets: Vec<String> = Vec::new();
            if !self.embedded_secret.is_empty() {
                secrets.push(self.embedded_secret.clone());
            }
            redact(message, &secrets) // Pass embedded_secret for custom secret redaction
        } else {
            message.to_string()
        };

        let sanitized_file_name = if self.redact_secrets && context.contains("embedded_secret") {
            "***REDACTED***".to_string()
        } else {
            file_name.to_string()
        };

        let line = format!(
            "{} [{}] {} — {}\n",
            timestamp, context, sanitized_file_name, sanitized_message
        );

        if let Ok(metadata) = fs::metadata(&*path) {
            if metadata.len() > self.max_size_mb * 1024 * 1024 {
                let rotated = path.with_extension("log.old");
                let _ = fs::rename(&*path, &rotated);
            }
        }

        #[cfg(unix)]
        {
            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .mode(SECURE_FILE_MODE)
                .open(&*path)
            {
                let _ = file.write_all(line.as_bytes());
            }
        }
        #[cfg(not(unix))]
        {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&*path) {
                let _ = file.write_all(line.as_bytes());
            }
        }
    }
}

/// Redact sensitive patterns from text.
/// Replaces embedded_secret values and common secret patterns with ***REDACTED***.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut result = text.to_string();

    // First: redact any custom secret values passed in
    for secret in secrets {
        if !secret.is_empty() {
            result = result.replace(secret, "***REDACTED***");
        }
    }

    // Then: apply keyword patterns
    let sensitive_patterns = [
        "embedded_secret",
        "secret=",
        "password=",
        "token=",
        "api_key=",
        "apikey=",
        "credential=",
        "auth=",
        "etc",
    ];
    for pattern in &sensitive_patterns {
        result = redact_pattern(&result, pattern);
    }

    result
}

/// Redact a single pattern and its associated value from text.
/// Pattern format: "keyword=" or just the keyword - replaces keyword and value with ***REDACTED***
fn redact_pattern(text: &str, pattern: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Check if pattern matches at current position
        if i + pattern.len() <= chars.len() {
            let slice: String = chars[i..i + pattern.len()].iter().collect();
            if slice == *pattern {
                result.push_str("***REDACTED***");

                // Skip past the pattern and find the end of the value (until space, newline, or end)
                i += pattern.len();

                while i < chars.len() {
                    let c = chars[i];
                    if c == ' ' || c == '\n' || c == '"' || c == '\'' {
                        break;
                    }
                    i += 1;
                }
                continue;
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_password() {
        let text = "Connection failed: password=mypassword123";
        let redacted = redact(text, &[]);
        // Should contain the redaction marker and NOT the actual value
        assert!(redacted.contains("***REDACTED***"));
        assert!(!redacted.contains("mypassword123"));
    }

    #[test]
    fn test_redact_token() {
        let text = "Auth token=abc123token456 failed";
        let redacted = redact(text, &[]);
        // Should contain the redaction marker
        assert!(redacted.contains("***REDACTED***"));
    }

    #[test]
    fn test_redact_embedded_secret() {
        let text = r#"embedded_secret: "mysecretvalue""#;
        let redacted = redact(text, &[]);
        assert!(redacted.contains("***REDACTED***"));
        // Should also work with the pattern variant
    }

    #[test]
    fn test_redact_multiple_secrets() {
        let text = "password=secret123 token=abc456";
        let redacted = redact(text, &[]);
        assert!(redacted.contains("***REDACTED***"));
        assert!(!redacted.contains("secret123"));
        assert!(!redacted.contains("abc456"));
    }

    #[test]
    fn test_no_secrets_in_clean_text() {
        let text = "File processed successfully";
        let redacted = redact(text, &[]);
        assert_eq!(text, redacted);
    }

    #[test]
    fn test_redact_replaces_entire_value_not_masked() {
        // The value after the pattern should be replaced entirely, not masked character by character
        let text = "password=verylongsecretvalue123";
        let redacted = redact(text, &[]);
        // Should be ***REDACTED*** followed by space or end, not *****************
        assert!(redacted.contains("***REDACTED***"));
        assert!(!redacted.contains("verylongsecretvalue123"));
        // Verify no partial masking with asterisks
        assert!(!redacted.contains("***************"));
    }

    #[test]
    fn test_redact_etc_path() {
        // Test that /etc/ paths are redacted
        let text = "Reading file from /etc/passwd for processing";
        let redacted = redact(text, &[]);
        assert!(redacted.contains("***REDACTED***"));
        assert!(!redacted.contains("/etc/passwd"));
    }
}
