use std::path::PathBuf;

use crate::config::watch::{
    AudioRule, CustomRule, DocumentRule, ImageRule, PdfRule, VideoRule,
};

pub enum MatchedRule {
    Video(VideoRule),
    Image(ImageRule),
    Audio(AudioRule),
    Pdf(PdfRule),
    Document(DocumentRule),
    Custom(CustomRule),
}

pub struct ConversionJob {
    pub watcher_name: String,
    pub file_name: String,
    pub file_path: PathBuf,
    pub matched_rule: MatchedRule,
    pub output_folder: String,
    pub watch_folder: String,
}
