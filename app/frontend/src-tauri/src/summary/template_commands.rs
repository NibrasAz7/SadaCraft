use crate::summary::{custom_templates, templates};
use serde::{Deserialize, Serialize};
use tauri::Runtime;
use tracing::{info, warn};

/// Template metadata for UI display
#[derive(Debug, Serialize, Deserialize)]
pub struct TemplateInfo {
    /// Template identifier (e.g., "daily_standup", "standard_meeting")
    pub id: String,

    /// Display name for the template
    pub name: String,

    /// Brief description of the template's purpose
    pub description: String,

    /// True when the template is a user-managed file in the app data
    /// templates directory (editable/deletable, overrides built-ins)
    pub is_custom: bool,
}

/// Full template content for the template editor
#[derive(Debug, Serialize, Deserialize)]
pub struct TemplateContent {
    /// Template identifier
    pub id: String,

    /// Display name
    pub name: String,

    /// Description
    pub description: String,

    /// Full section definitions (title, instruction, format, item formats)
    pub sections: Vec<templates::TemplateSection>,

    /// True when backed by a user-managed file (not a built-in template)
    pub is_custom: bool,
}

/// Detailed template structure for preview/debugging
#[derive(Debug, Serialize, Deserialize)]
pub struct TemplateDetails {
    /// Template identifier
    pub id: String,

    /// Display name
    pub name: String,

    /// Description
    pub description: String,

    /// List of section titles in order
    pub sections: Vec<String>,
}

/// Lists all available templates
///
/// Returns templates from both built-in (embedded) and custom (user data directory) sources.
/// Templates are automatically discovered - no code changes needed to add new templates.
///
/// # Returns
/// Vector of TemplateInfo with id, name, and description for each template
#[tauri::command]
pub async fn api_list_templates<R: Runtime>(
    _app: tauri::AppHandle<R>,
) -> Result<Vec<TemplateInfo>, String> {
    info!("api_list_templates called");

    let templates = templates::list_templates();

    let template_infos: Vec<TemplateInfo> = templates
        .into_iter()
        .map(|(id, name, description)| {
            let is_custom = custom_templates::custom_template_exists(&id);
            TemplateInfo {
                id,
                name,
                description,
                is_custom,
            }
        })
        .collect();

    info!("Found {} available templates", template_infos.len());

    Ok(template_infos)
}

/// Gets detailed information about a specific template
///
/// # Arguments
/// * `template_id` - Template identifier (e.g., "daily_standup")
///
/// # Returns
/// TemplateDetails with full template structure
#[tauri::command]
pub async fn api_get_template_details<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_id: String,
) -> Result<TemplateDetails, String> {
    info!("api_get_template_details called for template_id: {}", template_id);

    let template = templates::get_template(&template_id)?;

    let section_titles: Vec<String> = template
        .sections
        .iter()
        .map(|section| section.title.clone())
        .collect();

    let details = TemplateDetails {
        id: template_id,
        name: template.name,
        description: template.description,
        sections: section_titles,
    };

    info!("Retrieved template details for '{}'", details.name);

    Ok(details)
}

/// Validates a custom template JSON string
///
/// Useful for template editor UI or validation before saving custom templates
///
/// # Arguments
/// * `template_json` - Raw JSON string of the template
///
/// # Returns
/// Ok(template_name) if valid, Err(error_message) if invalid
#[tauri::command]
pub async fn api_validate_template<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_json: String,
) -> Result<String, String> {
    info!("api_validate_template called");

    match templates::validate_and_parse_template(&template_json) {
        Ok(template) => {
            info!("Template '{}' validated successfully", template.name);
            Ok(template.name)
        }
        Err(e) => {
            warn!("Template validation failed: {}", e);
            Err(e)
        }
    }
}

/// Gets the full content of a template (including section instructions and
/// formats) for the template editor
///
/// # Arguments
/// * `template_id` - Template identifier
///
/// # Returns
/// TemplateContent with the complete template structure
#[tauri::command]
pub async fn api_get_template_content<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_id: String,
) -> Result<TemplateContent, String> {
    info!("api_get_template_content called for template_id: {}", template_id);

    let template = templates::get_template(&template_id)?;

    Ok(TemplateContent {
        is_custom: custom_templates::custom_template_exists(&template_id),
        id: template_id,
        name: template.name,
        description: template.description,
        sections: template.sections,
    })
}

/// Creates a new user-managed template
///
/// The template is validated against the same schema the summary engine
/// consumes and written to the app data templates directory. The id must be
/// unique across built-in, bundled, and custom templates — the frontend uses
/// a `custom-` prefix for user-created templates.
///
/// # Arguments
/// * `template_id` - Template identifier (letters, digits, '-', '_')
/// * `template_json` - Raw JSON string of the template
///
/// # Returns
/// TemplateInfo for the created template
#[tauri::command]
pub async fn api_create_template<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_id: String,
    template_json: String,
) -> Result<TemplateInfo, String> {
    info!("api_create_template called for template_id: {}", template_id);

    custom_templates::validate_template_id(&template_id)?;

    if templates::list_template_ids().contains(&template_id) {
        return Err(format!(
            "A template with id '{}' already exists",
            template_id
        ));
    }

    let template = templates::validate_and_parse_template(&template_json)?;

    let path = custom_templates::save_custom_template(&template_id, &template_json)?;
    info!("Created custom template '{}' at {:?}", template_id, path);

    Ok(TemplateInfo {
        id: template_id,
        name: template.name,
        description: template.description,
        is_custom: true,
    })
}

/// Saves a template by writing it to the user templates directory
///
/// For a custom template this updates the existing file. For a built-in or
/// bundled template id this creates a user-managed override — the loader
/// checks the custom directory first, so the saved version wins from then on.
/// To revert an override, delete the custom template.
///
/// # Arguments
/// * `template_id` - Template identifier to save under
/// * `template_json` - Raw JSON string of the template
///
/// # Returns
/// TemplateInfo for the saved template
#[tauri::command]
pub async fn api_update_template<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_id: String,
    template_json: String,
) -> Result<TemplateInfo, String> {
    info!("api_update_template called for template_id: {}", template_id);

    custom_templates::validate_template_id(&template_id)?;

    let template = templates::validate_and_parse_template(&template_json)?;

    let path = custom_templates::save_custom_template(&template_id, &template_json)?;
    info!("Saved custom template '{}' at {:?}", template_id, path);

    Ok(TemplateInfo {
        id: template_id,
        name: template.name,
        description: template.description,
        is_custom: true,
    })
}

/// Deletes a user-managed template file
///
/// Only files in the app data templates directory can be deleted; built-in
/// and bundled templates cannot. When a custom override shares an id with a
/// built-in template, deleting it reverts to the built-in version.
///
/// # Arguments
/// * `template_id` - Template identifier
#[tauri::command]
pub async fn api_delete_template<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_id: String,
) -> Result<(), String> {
    info!("api_delete_template called for template_id: {}", template_id);

    match custom_templates::delete_custom_template(&template_id)? {
        true => {
            info!("Deleted custom template '{}'", template_id);
            Ok(())
        }
        false => Err(format!(
            "No custom template with id '{}' exists. Built-in templates cannot be deleted.",
            template_id
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_list_templates() {
        // This test requires the templates to be embedded/available
        // In a real test environment, you might want to mock the templates module

        // For now, just verify the function compiles and runs
        // You can expand this with more specific assertions
    }

    #[tokio::test]
    async fn test_validate_template_valid() {
        let valid_json = r#"
        {
            "name": "Test Template",
            "description": "A test template",
            "sections": [
                {
                    "title": "Summary",
                    "instruction": "Provide a summary",
                    "format": "paragraph"
                }
            ]
        }"#;

        // Mock app handle would be needed for actual testing
        // For now, test the validation logic directly
        let result = templates::validate_and_parse_template(valid_json);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_template_invalid() {
        let invalid_json = "invalid json";

        let result = templates::validate_and_parse_template(invalid_json);
        assert!(result.is_err());
    }
}
