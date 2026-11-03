use serde::{Deserialize, Serialize};

use super::watch::WatchType;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EmbeddedConfig {
    pub secret: String,

    #[serde(default)]
    pub output_folder: String,

    #[serde(flatten)]
    pub watch_type: WatchType,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_override_video_yaml() {
        let yaml = r#"---
secret: ""
output_folder: /path/to/output/override
video:
  rules:
    - input_extensions: [.mp4, .mkv]
      preset: libx264
"#;
        let parsed: EmbeddedConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.secret, "");
        assert_eq!(parsed.output_folder, "/path/to/output/override");
        match &parsed.watch_type {
            WatchType::Video { rules } => {
                assert_eq!(rules.len(), 1);
                assert_eq!(rules[0].preset, "libx264");
            }
            _ => panic!("Expected Video type"),
        }
    }

    #[test]
    fn test_embedded_override_image_yaml() {
        let yaml = r#"---
secret: my-secret
output_folder: /tmp/output
image:
  rules:
    - input_extensions: [.tiff, .png]
      preset: jpeg_80
"#;
        let parsed: EmbeddedConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.secret, "my-secret");
        match &parsed.watch_type {
            WatchType::Image { rules } => {
                assert_eq!(rules.len(), 1);
                assert_eq!(rules[0].preset, "jpeg_80");
            }
            _ => panic!("Expected Image type"),
        }
    }

    #[test]
    fn test_embedded_override_audio_yaml() {
        let yaml = r#"---
secret: "cw-1234567890"
audio:
  rules:
    - input_extensions: [.wav, .flac]
      preset: mp3_192
"#;
        let parsed: EmbeddedConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.secret, "cw-1234567890");
        match &parsed.watch_type {
            WatchType::Audio { rules } => {
                assert_eq!(rules.len(), 1);
                assert_eq!(rules[0].preset, "mp3_192");
            }
            _ => panic!("Expected Audio type"),
        }
    }

    #[test]
    fn test_embedded_override_with_subfolder() {
        let yaml = r#"---
secret: ""
video:
  rules:
    - subfolder: gpu
      input_extensions: [.mxf, .mov]
      preset: h264_nvenc
"#;
        let parsed: EmbeddedConfig = serde_yaml::from_str(yaml).unwrap();
        match &parsed.watch_type {
            WatchType::Video { rules } => {
                assert!(rules[0].subfolder.is_some());
                assert_eq!(rules[0].subfolder.as_ref().unwrap(), "gpu");
            }
            _ => panic!("Expected Video type"),
        }
    }

    #[test]
    fn test_embedded_override_empty_output_folder() {
        let yaml = r#"---
secret: ""
output_folder: ""
video:
  rules: []
"#;
        let parsed: EmbeddedConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(parsed.output_folder.is_empty());
    }

    #[test]
    fn test_embedded_override_missing_secret() {
        // secret is required by the struct, so this should fail if not provided
        let yaml = r#"---
output_folder: /tmp/output
video:
  rules: []
"#;
        let result: Result<EmbeddedConfig, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "secret field is required");
    }

    #[test]
    fn test_embedded_override_pdf_type() {
        let yaml = r#"---
secret: ""
pdf:
  rules:
    - input_extensions: [.pdf]
      preset: pdf_ebook
"#;
        let parsed: EmbeddedConfig = serde_yaml::from_str(yaml).unwrap();
        match &parsed.watch_type {
            WatchType::Pdf { .. } => {}
            _ => panic!("Expected Pdf type"),
        }
    }

    #[test]
    fn test_embedded_override_document_type() {
        let yaml = r#"---
secret: ""
document:
  rules:
    - input_extensions: [.docx]
      preset: docx_to_pdf
"#;
        let parsed: EmbeddedConfig = serde_yaml::from_str(yaml).unwrap();
        match &parsed.watch_type {
            WatchType::Document { .. } => {}
            _ => panic!("Expected Document type"),
        }
    }

    #[test]
    fn test_embedded_override_custom_type() {
        let yaml = r#"---
secret: ""
custom:
  rules:
    - input_extensions: [.mkv]
      preset: handbrake
"#;
        let parsed: EmbeddedConfig = serde_yaml::from_str(yaml).unwrap();
        match &parsed.watch_type {
            WatchType::Custom { .. } => {}
            _ => panic!("Expected Custom type"),
        }
    }
}
