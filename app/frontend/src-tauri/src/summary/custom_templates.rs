//! Filesystem helpers for user-managed summary templates.
//!
//! Custom templates are stored in the platform data directory under
//! `SadaCraft/templates/` — the same location `templates::loader` reads from.
//! Because the loader checks the custom directory first, a user template with
//! the same `template_id` overrides the bundled/built-in template.
//!
//! - macOS: `~/Library/Application Support/SadaCraft/templates/`
//! - Windows: `%APPDATA%\SadaCraft\templates\`
//! - Linux: `~/.config/SadaCraft/templates/`

use std::path::PathBuf;

/// Maximum allowed length for a template identifier.
const MAX_TEMPLATE_ID_LEN: usize = 64;

/// Returns the user's custom templates directory.
fn custom_templates_dir() -> Result<PathBuf, String> {
    let mut path = dirs::data_dir()
        .ok_or_else(|| "Could not resolve the application data directory".to_string())?;
    path.push("SadaCraft");
    path.push("templates");
    Ok(path)
}

/// Validates a template identifier for use as a file name.
///
/// Allowed characters: ASCII letters, digits, '-' and '_'.
pub fn validate_template_id(template_id: &str) -> Result<(), String> {
    if template_id.is_empty() {
        return Err("Template id cannot be empty".to_string());
    }
    if template_id.len() > MAX_TEMPLATE_ID_LEN {
        return Err(format!(
            "Template id cannot exceed {} characters",
            MAX_TEMPLATE_ID_LEN
        ));
    }
    if !template_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Template id may only contain letters, digits, '-' and '_'".to_string());
    }
    Ok(())
}

/// Path of a custom template file inside the user templates directory.
fn custom_template_path(template_id: &str) -> Result<PathBuf, String> {
    validate_template_id(template_id)?;
    Ok(custom_templates_dir()?.join(format!("{}.json", template_id)))
}

/// Returns true when a user-managed template file exists for `template_id`.
pub fn custom_template_exists(template_id: &str) -> bool {
    custom_template_path(template_id)
        .map(|path| path.is_file())
        .unwrap_or(false)
}

/// Writes template JSON to the user templates directory, creating it when needed.
pub fn save_custom_template(template_id: &str, template_json: &str) -> Result<PathBuf, String> {
    let dir = custom_templates_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create custom templates directory: {}", e))?;

    let path = custom_template_path(template_id)?;
    std::fs::write(&path, template_json)
        .map_err(|e| format!("Failed to write template file: {}", e))?;

    Ok(path)
}

/// Deletes a user-managed template file.
/// Returns false when no custom file exists for `template_id`.
pub fn delete_custom_template(template_id: &str) -> Result<bool, String> {
    let path = custom_template_path(template_id)?;
    if !path.is_file() {
        return Ok(false);
    }

    std::fs::remove_file(&path).map_err(|e| format!("Failed to delete template file: {}", e))?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_template_id_valid() {
        assert!(validate_template_id("custom-my_template-01").is_ok());
    }

    #[test]
    fn test_validate_template_id_rejects_empty() {
        assert!(validate_template_id("").is_err());
    }

    #[test]
    fn test_validate_template_id_rejects_path_separators() {
        assert!(validate_template_id("../evil").is_err());
        assert!(validate_template_id("a/b").is_err());
        assert!(validate_template_id("a\\b").is_err());
    }

    #[test]
    fn test_validate_template_id_rejects_long_ids() {
        assert!(validate_template_id(&"x".repeat(65)).is_err());
    }
}
