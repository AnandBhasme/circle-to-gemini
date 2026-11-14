// Configuration: API key, settings, persistence

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub gemini_api_key: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_mode")]
    pub mode: String, // "smart" or "fast"
    #[serde(default)]
    pub debug_logging: bool, // Default is false when shipped
    #[serde(default = "default_single_window")]
    pub single_window_mode: bool, // Close previous assistant when new one opens
    #[serde(default = "default_system_instruction")]
    pub system_instruction: String,
    #[serde(default)]
    pub window_x: Option<f32>,
    #[serde(default)]
    pub window_y: Option<f32>,
    #[serde(default)]
    pub window_width: Option<f32>,
    #[serde(default)]
    pub window_height: Option<f32>,
}

fn default_model() -> String {
    "gemini-3.8-flash".to_string()
}

fn default_mode() -> String {
    "smart".to_string()
}

fn default_single_window() -> bool {
    true
}

pub fn default_system_instruction() -> String {
    "You are a helpful visual assistant. Format your response in clean Markdown. For mathematical formulas, equations, and matrices, use standard LaTeX notation inside display math ($$ ... $$) or inline math ($ ... $). For matrices and game payoff tables, use standard LaTeX matrix environments (such as \\begin{pmatrix} ... \\end{pmatrix} or \\begin{bmatrix} ... \\end{bmatrix}) or clean markdown tables inside display blocks. Always include row and column labels clearly.".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            gemini_api_key: String::new(),
            model: default_model(),
            mode: default_mode(),
            debug_logging: false,
            single_window_mode: true,
            system_instruction: default_system_instruction(),
            window_x: None,
            window_y: None,
            window_width: None,
            window_height: None,
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        let dirs = ProjectDirs::from("dev", "ctg", "circle-to-gemini")
            .expect("Could not determine config directory");
        let config_dir = dirs.config_dir();
        fs::create_dir_all(config_dir).ok();
        config_dir.join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            let data = fs::read_to_string(&path).unwrap_or_default();
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            let cfg = Self::default();
            cfg.save();
            cfg
        }
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Ok(data) = serde_json::to_string_pretty(self) {
            fs::write(path, data).ok();
        }
    }

    pub fn has_api_key(&self) -> bool {
        !self.gemini_api_key.trim().is_empty()
    }
}
