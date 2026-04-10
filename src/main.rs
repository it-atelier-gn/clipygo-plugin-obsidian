use std::fs;
use std::io::{self, BufRead, Write as IoWrite};
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Local;
use serde::{Deserialize, Serialize};

// --- Protocol types ---

#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
#[allow(dead_code)]
enum Request {
    GetInfo,
    GetTargets,
    GetConfigSchema,
    SetConfig {
        values: serde_json::Value,
    },
    Send {
        target_id: String,
        content: String,
        format: String,
    },
}

#[derive(Serialize)]
struct InfoResponse {
    name: &'static str,
    version: &'static str,
    description: &'static str,
    author: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    link: Option<&'static str>,
}

#[derive(Serialize)]
struct Target {
    id: String,
    provider: &'static str,
    formats: Vec<&'static str>,
    title: String,
    description: String,
    image: &'static str,
}

#[derive(Serialize)]
struct TargetsResponse {
    targets: Vec<Target>,
}

#[derive(Serialize)]
struct SendResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

// --- Config ---

#[derive(Clone, Serialize, Deserialize)]
struct ObsidianConfig {
    vault_path: String,
    daily_notes_folder: String,
    daily_notes_format: String,
    inbox_note: String,
    append_template: String,
}

impl Default for ObsidianConfig {
    fn default() -> Self {
        Self {
            vault_path: detect_vault_path().unwrap_or_default(),
            daily_notes_folder: "Daily Notes".to_string(),
            daily_notes_format: "%Y-%m-%d".to_string(),
            inbox_note: "Inbox.md".to_string(),
            append_template: "\n---\n*{timestamp}*\n{content}\n".to_string(),
        }
    }
}

static CONFIG: Mutex<Option<ObsidianConfig>> = Mutex::new(None);
static CONFIG_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("clipygo-plugin-obsidian"))
}

fn config_file_path() -> Option<PathBuf> {
    let cached = CONFIG_PATH.lock().unwrap();
    if let Some(ref p) = *cached {
        return Some(p.clone());
    }
    drop(cached);

    let path = config_dir()?.join("config.json");
    *CONFIG_PATH.lock().unwrap() = Some(path.clone());
    Some(path)
}

fn load_config() -> ObsidianConfig {
    if let Some(path) = config_file_path() {
        if let Ok(data) = fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<ObsidianConfig>(&data) {
                return cfg;
            }
        }
    }
    ObsidianConfig::default()
}

fn save_config(config: &ObsidianConfig) {
    if let Some(path) = config_file_path() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, serde_json::to_string_pretty(config).unwrap());
    }
}

fn get_config() -> ObsidianConfig {
    let guard = CONFIG.lock().unwrap();
    guard.clone().unwrap_or_else(|| {
        drop(guard);
        let cfg = load_config();
        *CONFIG.lock().unwrap() = Some(cfg.clone());
        cfg
    })
}

/// Try to detect the default Obsidian vault path from Obsidian's config.
fn detect_vault_path() -> Option<String> {
    let obsidian_json = if cfg!(target_os = "windows") {
        dirs::config_dir()?.join("obsidian").join("obsidian.json")
    } else if cfg!(target_os = "macos") {
        dirs::home_dir()?
            .join("Library")
            .join("Application Support")
            .join("obsidian")
            .join("obsidian.json")
    } else {
        dirs::config_dir()?.join("obsidian").join("obsidian.json")
    };

    let data = fs::read_to_string(&obsidian_json).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&data).ok()?;
    let vaults = parsed.get("vaults")?.as_object()?;

    // Return the first vault's path (most users have one vault)
    for (_id, vault) in vaults {
        if let Some(path) = vault.get("path").and_then(|v| v.as_str()) {
            return Some(path.to_string());
        }
    }
    None
}

// --- Note writing ---

fn resolve_vault_path(config: &ObsidianConfig) -> Result<PathBuf, String> {
    let path = PathBuf::from(&config.vault_path);
    if config.vault_path.is_empty() {
        return Err("Vault path not configured".to_string());
    }
    if !path.is_dir() {
        return Err(format!("Vault not found: {}", config.vault_path));
    }
    Ok(path)
}

fn format_entry(config: &ObsidianConfig, content: &str) -> String {
    let now = Local::now();
    config
        .append_template
        .replace("{timestamp}", &now.format("%Y-%m-%d %H:%M").to_string())
        .replace("{content}", content)
}

fn append_to_note(note_path: &PathBuf, entry: &str) -> Result<(), String> {
    if let Some(parent) = note_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {e}"))?;
    }

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(note_path)
        .map_err(|e| format!("Failed to open note: {e}"))?;

    file.write_all(entry.as_bytes())
        .map_err(|e| format!("Failed to write to note: {e}"))?;

    Ok(())
}

fn daily_note_path(config: &ObsidianConfig) -> Result<PathBuf, String> {
    let vault = resolve_vault_path(config)?;
    let today = Local::now().format(&config.daily_notes_format).to_string();
    let filename = format!("{today}.md");
    Ok(vault.join(&config.daily_notes_folder).join(filename))
}

fn inbox_note_path(config: &ObsidianConfig) -> Result<PathBuf, String> {
    let vault = resolve_vault_path(config)?;
    Ok(vault.join(&config.inbox_note))
}

fn create_new_note(config: &ObsidianConfig, content: &str) -> Result<PathBuf, String> {
    let vault = resolve_vault_path(config)?;
    let first_line = content.lines().next().unwrap_or("Untitled");
    // Sanitize filename
    let safe_name: String = first_line
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe_name = safe_name.trim();
    let name = if safe_name.is_empty() {
        Local::now().format("Note %Y-%m-%d %H%M%S").to_string()
    } else if safe_name.len() > 80 {
        safe_name[..80].trim().to_string()
    } else {
        safe_name.to_string()
    };

    let note_path = vault.join(format!("{name}.md"));
    if note_path.exists() {
        // Append timestamp to avoid overwriting
        let ts = Local::now().format("%H%M%S").to_string();
        let note_path = vault.join(format!("{name} {ts}.md"));
        fs::write(&note_path, content).map_err(|e| format!("Failed to create note: {e}"))?;
        return Ok(note_path);
    }
    fs::write(&note_path, content).map_err(|e| format!("Failed to create note: {e}"))?;
    Ok(note_path)
}

// --- Handlers ---

// 1x1 purple pixel PNG for the icon
const ICON: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M/wHwAEBgIApD5fRAAAAABJRU5ErkJggg==";

fn handle(request: Request) -> serde_json::Value {
    match request {
        Request::GetInfo => serde_json::to_value(InfoResponse {
            name: "Obsidian",
            version: env!("CARGO_PKG_VERSION"),
            description: "Send clipboard content to your Obsidian vault",
            author: "clipygo",
            link: Some("https://github.com/it-atelier-gn/clipygo-plugin-obsidian"),
        })
        .unwrap(),

        Request::GetTargets => {
            let config = get_config();
            let vault_ok = resolve_vault_path(&config).is_ok();

            let mut targets = Vec::new();
            if vault_ok {
                targets.push(Target {
                    id: "daily-note".to_string(),
                    provider: "Obsidian",
                    formats: vec!["text"],
                    title: "Daily Note".to_string(),
                    description: "Append to today's daily note".to_string(),
                    image: ICON,
                });
                targets.push(Target {
                    id: "inbox".to_string(),
                    provider: "Obsidian",
                    formats: vec!["text"],
                    title: "Inbox".to_string(),
                    description: format!("Append to {}", config.inbox_note),
                    image: ICON,
                });
                targets.push(Target {
                    id: "new-note".to_string(),
                    provider: "Obsidian",
                    formats: vec!["text"],
                    title: "New Note".to_string(),
                    description: "Create a new note from clipboard".to_string(),
                    image: ICON,
                });
            }
            serde_json::to_value(TargetsResponse { targets }).unwrap()
        }

        Request::GetConfigSchema => {
            let config = get_config();
            serde_json::json!({
                "instructions": "Configure your Obsidian vault path and note settings.\n\
                    The vault path is auto-detected from Obsidian's config if possible.",
                "schema": {
                    "type": "object",
                    "title": "Obsidian Plugin",
                    "properties": {
                        "vault_path": {
                            "type": "string",
                            "title": "Vault Path",
                            "description": "Path to your Obsidian vault directory"
                        },
                        "daily_notes_folder": {
                            "type": "string",
                            "title": "Daily Notes Folder",
                            "description": "Subfolder for daily notes (relative to vault root)"
                        },
                        "daily_notes_format": {
                            "type": "string",
                            "title": "Daily Notes Format",
                            "description": "Date format for daily note filenames (strftime)"
                        },
                        "inbox_note": {
                            "type": "string",
                            "title": "Inbox Note",
                            "description": "Filename for the inbox note (relative to vault root)"
                        },
                        "append_template": {
                            "type": "string",
                            "title": "Append Template",
                            "description": "Template for appended entries. Use {timestamp} and {content}"
                        }
                    }
                },
                "values": {
                    "vault_path": config.vault_path,
                    "daily_notes_folder": config.daily_notes_folder,
                    "daily_notes_format": config.daily_notes_format,
                    "inbox_note": config.inbox_note,
                    "append_template": config.append_template
                }
            })
        }

        Request::SetConfig { values } => {
            let mut config = get_config();
            if let Some(v) = values.get("vault_path").and_then(|v| v.as_str()) {
                config.vault_path = v.to_string();
            }
            if let Some(v) = values.get("daily_notes_folder").and_then(|v| v.as_str()) {
                config.daily_notes_folder = v.to_string();
            }
            if let Some(v) = values.get("daily_notes_format").and_then(|v| v.as_str()) {
                config.daily_notes_format = v.to_string();
            }
            if let Some(v) = values.get("inbox_note").and_then(|v| v.as_str()) {
                config.inbox_note = v.to_string();
            }
            if let Some(v) = values.get("append_template").and_then(|v| v.as_str()) {
                config.append_template = v.to_string();
            }
            save_config(&config);
            *CONFIG.lock().unwrap() = Some(config);
            serde_json::to_value(SendResponse {
                success: true,
                error: None,
            })
            .unwrap()
        }

        Request::Send {
            target_id,
            content,
            format: _,
        } => {
            let config = get_config();
            let result = match target_id.as_str() {
                "daily-note" => {
                    let entry = format_entry(&config, &content);
                    daily_note_path(&config).and_then(|p| append_to_note(&p, &entry))
                }
                "inbox" => {
                    let entry = format_entry(&config, &content);
                    inbox_note_path(&config).and_then(|p| append_to_note(&p, &entry))
                }
                "new-note" => create_new_note(&config, &content).map(|_| ()),
                _ => Err(format!("Unknown target: {target_id}")),
            };

            match result {
                Ok(()) => serde_json::to_value(SendResponse {
                    success: true,
                    error: None,
                })
                .unwrap(),
                Err(e) => serde_json::to_value(SendResponse {
                    success: false,
                    error: Some(e),
                })
                .unwrap(),
            }
        }
    }
}

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handle(request),
            Err(e) => serde_json::json!({ "error": format!("Bad request: {}", e) }),
        };

        let _ = writeln!(out, "{response}");
        let _ = out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn with_temp_vault<F: FnOnce(PathBuf)>(f: F) {
        let dir = env::temp_dir().join(format!("clipygo-obsidian-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        f(dir.clone());
        let _ = fs::remove_dir_all(&dir);
    }

    fn test_config(vault: &PathBuf) -> ObsidianConfig {
        ObsidianConfig {
            vault_path: vault.to_string_lossy().to_string(),
            daily_notes_folder: "Daily Notes".to_string(),
            daily_notes_format: "%Y-%m-%d".to_string(),
            inbox_note: "Inbox.md".to_string(),
            append_template: "\n---\n*{timestamp}*\n{content}\n".to_string(),
        }
    }

    #[test]
    fn get_info_returns_name_and_version() {
        let resp = handle(Request::GetInfo);
        assert_eq!(resp["name"], "Obsidian");
        assert!(resp["version"].is_string());
    }

    #[test]
    fn get_targets_empty_when_no_vault() {
        *CONFIG.lock().unwrap() = Some(ObsidianConfig {
            vault_path: "/nonexistent/path".to_string(),
            ..ObsidianConfig::default()
        });
        let resp = handle(Request::GetTargets);
        let targets = resp["targets"].as_array().unwrap();
        assert!(targets.is_empty());
    }

    #[test]
    fn get_targets_returns_three_when_vault_exists() {
        with_temp_vault(|vault| {
            *CONFIG.lock().unwrap() = Some(test_config(&vault));
            let resp = handle(Request::GetTargets);
            let targets = resp["targets"].as_array().unwrap();
            assert_eq!(targets.len(), 3);
            assert_eq!(targets[0]["id"], "daily-note");
            assert_eq!(targets[1]["id"], "inbox");
            assert_eq!(targets[2]["id"], "new-note");
        });
    }

    #[test]
    fn send_to_daily_note_creates_file() {
        with_temp_vault(|vault| {
            *CONFIG.lock().unwrap() = Some(test_config(&vault));
            let resp = handle(Request::Send {
                target_id: "daily-note".to_string(),
                content: "test content".to_string(),
                format: "text".to_string(),
            });
            assert_eq!(resp["success"], true);

            let today = Local::now().format("%Y-%m-%d").to_string();
            let note = vault.join("Daily Notes").join(format!("{today}.md"));
            assert!(note.exists());
            let body = fs::read_to_string(&note).unwrap();
            assert!(body.contains("test content"));
        });
    }

    #[test]
    fn send_to_inbox_appends() {
        with_temp_vault(|vault| {
            *CONFIG.lock().unwrap() = Some(test_config(&vault));

            handle(Request::Send {
                target_id: "inbox".to_string(),
                content: "first".to_string(),
                format: "text".to_string(),
            });
            handle(Request::Send {
                target_id: "inbox".to_string(),
                content: "second".to_string(),
                format: "text".to_string(),
            });

            let note = vault.join("Inbox.md");
            let body = fs::read_to_string(&note).unwrap();
            assert!(body.contains("first"));
            assert!(body.contains("second"));
        });
    }

    #[test]
    fn send_new_note_uses_first_line_as_filename() {
        with_temp_vault(|vault| {
            *CONFIG.lock().unwrap() = Some(test_config(&vault));
            let resp = handle(Request::Send {
                target_id: "new-note".to_string(),
                content: "My Great Idea\nSome details here".to_string(),
                format: "text".to_string(),
            });
            assert_eq!(resp["success"], true);

            let note = vault.join("My Great Idea.md");
            assert!(note.exists());
            let body = fs::read_to_string(&note).unwrap();
            assert!(body.contains("Some details here"));
        });
    }

    #[test]
    fn send_new_note_sanitizes_filename() {
        with_temp_vault(|vault| {
            *CONFIG.lock().unwrap() = Some(test_config(&vault));
            let resp = handle(Request::Send {
                target_id: "new-note".to_string(),
                content: "What/about:these<chars>?".to_string(),
                format: "text".to_string(),
            });
            assert_eq!(resp["success"], true);

            // Should not contain special chars in filename
            let entries: Vec<_> = fs::read_dir(&vault)
                .unwrap()
                .filter_map(|e| e.ok())
                .collect();
            assert_eq!(entries.len(), 1);
            let name = entries[0].file_name().to_string_lossy().to_string();
            assert!(!name.contains('/'));
            assert!(!name.contains(':'));
            assert!(!name.contains('<'));
        });
    }

    #[test]
    fn send_fails_when_vault_missing() {
        *CONFIG.lock().unwrap() = Some(ObsidianConfig {
            vault_path: "/nonexistent/vault".to_string(),
            ..ObsidianConfig::default()
        });
        let resp = handle(Request::Send {
            target_id: "inbox".to_string(),
            content: "test".to_string(),
            format: "text".to_string(),
        });
        assert_eq!(resp["success"], false);
        assert!(resp["error"].as_str().unwrap().contains("not found"));
    }

    #[test]
    fn get_config_schema_returns_values() {
        *CONFIG.lock().unwrap() = Some(ObsidianConfig::default());
        let resp = handle(Request::GetConfigSchema);
        assert!(resp.get("schema").is_some());
        assert!(resp.get("values").is_some());
        assert_eq!(resp["values"]["daily_notes_folder"], "Daily Notes");
    }

    #[test]
    fn set_config_persists_values() {
        *CONFIG.lock().unwrap() = Some(ObsidianConfig::default());
        let resp = handle(Request::SetConfig {
            values: serde_json::json!({
                "inbox_note": "Captures.md",
                "daily_notes_folder": "Journal"
            }),
        });
        assert_eq!(resp["success"], true);

        let config = get_config();
        assert_eq!(config.inbox_note, "Captures.md");
        assert_eq!(config.daily_notes_folder, "Journal");
    }

    #[test]
    fn format_entry_substitutes_placeholders() {
        let config = ObsidianConfig {
            append_template: "\n- {timestamp}: {content}\n".to_string(),
            ..ObsidianConfig::default()
        };
        let entry = format_entry(&config, "hello world");
        assert!(entry.contains("hello world"));
        // Should have a date-like timestamp
        assert!(entry.contains(&Local::now().format("%Y").to_string()));
    }

    #[test]
    fn detect_vault_path_returns_none_when_no_obsidian() {
        // This test just ensures it doesn't panic
        let _ = detect_vault_path();
    }
}
