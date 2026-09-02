//! Application configuration loaded from the writable data directory.
//!
//! `config.json` contains protocol metadata only. Credentials and signed URLs
//! belong in the DPAPI-backed credential store and must never be added here.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::providers::{
    default_oauth_configs, GuangyaConfig, OAuthProviderConfig, StreamHubConfig,
};

pub const CONFIG_FILE_NAME: &str = "config.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub guangya: GuangyaConfig,
    pub streamhub: StreamHubConfig,
    #[serde(default = "default_oauth_configs")]
    pub oauth: std::collections::BTreeMap<String, OAuthProviderConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            guangya: GuangyaConfig::default(),
            streamhub: StreamHubConfig::default(),
            oauth: default_oauth_configs(),
        }
    }
}

impl AppConfig {
    pub fn load(data_dir: &Path) -> Result<Self, AppError> {
        let path = data_dir.join(CONFIG_FILE_NAME);
        let mut config = match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).map_err(|error| {
                AppError::Runtime(format!(
                    "invalid configuration file {}: {error}",
                    path.display()
                ))
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let config = Self::default();
                config.write_template(&path)?;
                config
            }
            Err(error) => {
                return Err(AppError::Runtime(format!(
                    "cannot read configuration file {}: {error}",
                    path.display()
                )))
            }
        };
        config.apply_environment_overrides(|key| std::env::var(key));
        Ok(config)
    }

    /// Creates an editable first-run configuration file. Guangya uses the
    /// provider's public web OAuth client for QR login; account tokens stay in
    /// CredentialStore.
    fn write_template(&self, path: &Path) -> Result<(), AppError> {
        let parent = path.parent().ok_or_else(|| {
            AppError::Runtime(format!(
                "configuration path has no parent: {}",
                path.display()
            ))
        })?;
        std::fs::create_dir_all(parent).map_err(|error| {
            AppError::Runtime(format!(
                "cannot create configuration directory {}: {error}",
                parent.display()
            ))
        })?;
        let contents = serde_json::to_string_pretty(self).map_err(|error| {
            AppError::Runtime(format!("cannot serialize default configuration: {error}"))
        })?;
        std::fs::write(path, format!("{contents}\n")).map_err(|error| {
            AppError::Runtime(format!(
                "cannot create configuration file {}: {error}",
                path.display()
            ))
        })
    }

    fn apply_environment_overrides<F, E>(&mut self, lookup: F)
    where
        F: Fn(&str) -> Result<String, E>,
    {
        apply_string_override(
            &mut self.guangya.account_base_url,
            "TTV_GUANGYA_ACCOUNT_BASE_URL",
            &lookup,
        );
        apply_string_override(
            &mut self.guangya.api_base_url,
            "TTV_GUANGYA_API_BASE_URL",
            &lookup,
        );
        apply_string_override(
            &mut self.guangya.device_code_path,
            "TTV_GUANGYA_DEVICE_CODE_PATH",
            &lookup,
        );
        apply_string_override(
            &mut self.guangya.token_path,
            "TTV_GUANGYA_TOKEN_PATH",
            &lookup,
        );
        apply_option_override(
            &mut self.guangya.client_id,
            "TTV_GUANGYA_CLIENT_ID",
            &lookup,
        );
        apply_option_override(
            &mut self.guangya.oauth_client_id,
            "TTV_GUANGYA_OAUTH_CLIENT_ID",
            &lookup,
        );
        apply_option_override(&mut self.guangya.scope, "TTV_GUANGYA_SCOPE", &lookup);
        apply_option_override(
            &mut self.guangya.device_code_grant_type,
            "TTV_GUANGYA_DEVICE_CODE_GRANT_TYPE",
            &lookup,
        );
        apply_option_override(
            &mut self.guangya.refresh_grant_type,
            "TTV_GUANGYA_REFRESH_GRANT_TYPE",
            &lookup,
        );
        apply_option_override(
            &mut self.guangya.user_agent,
            "TTV_GUANGYA_USER_AGENT",
            &lookup,
        );
        apply_string_override(
            &mut self.streamhub.base_url,
            "TTV_STREAMHUB_BASE_URL",
            &lookup,
        );
        apply_option_override(
            &mut self.streamhub.user_agent,
            "TTV_STREAMHUB_USER_AGENT",
            &lookup,
        );

        for (provider_id, oauth) in &mut self.oauth {
            let key_id = provider_id.to_ascii_uppercase().replace('-', "_");
            apply_option_override(
                &mut oauth.client_id,
                &format!("TTV_OAUTH_{key_id}_CLIENT_ID"),
                &lookup,
            );
            apply_option_override(
                &mut oauth.client_secret,
                &format!("TTV_OAUTH_{key_id}_CLIENT_SECRET"),
                &lookup,
            );
            apply_string_override(
                &mut oauth.redirect_uri,
                &format!("TTV_OAUTH_{key_id}_REDIRECT_URI"),
                &lookup,
            );
            apply_option_override(
                &mut oauth.scope,
                &format!("TTV_OAUTH_{key_id}_SCOPE"),
                &lookup,
            );
            apply_option_override(
                &mut oauth.device_code_endpoint,
                &format!("TTV_OAUTH_{key_id}_DEVICE_CODE_ENDPOINT"),
                &lookup,
            );
            apply_option_override(
                &mut oauth.device_code_grant_type,
                &format!("TTV_OAUTH_{key_id}_DEVICE_CODE_GRANT_TYPE"),
                &lookup,
            );
            apply_option_override(
                &mut oauth.refresh_grant_type,
                &format!("TTV_OAUTH_{key_id}_REFRESH_GRANT_TYPE"),
                &lookup,
            );
        }
    }
}

fn apply_string_override<F, E>(target: &mut String, key: &str, lookup: &F)
where
    F: Fn(&str) -> Result<String, E>,
{
    if let Ok(value) = lookup(key) {
        *target = value;
    }
}

fn apply_option_override<F, E>(target: &mut Option<String>, key: &str, lookup: &F)
where
    F: Fn(&str) -> Result<String, E>,
{
    if let Ok(value) = lookup(key) {
        *target = (!value.trim().is_empty()).then_some(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_enable_web_guangya_oauth_without_developer_credentials() {
        let config = AppConfig::default();
        assert!(config.guangya.oauth_configured());
        assert!(config.guangya.client_id.is_none());
        assert_eq!(
            config.guangya.oauth_client_id.as_deref(),
            Some("aMe-8VSlkrbQXpUR")
        );
        assert_eq!(
            config.guangya.device_code_grant_type.as_deref(),
            Some("urn:ietf:params:oauth:grant-type:device_code")
        );
        assert_eq!(
            config.guangya.refresh_grant_type.as_deref(),
            Some("refresh_token")
        );
    }

    #[test]
    fn environment_overrides_take_precedence_and_empty_values_clear_options() {
        let mut config = AppConfig::default();
        config.guangya.client_id = Some("from-file".into());
        config.apply_environment_overrides(|key| match key {
            "TTV_GUANGYA_ACCOUNT_BASE_URL" => Ok::<_, ()>("https://example.invalid".into()),
            "TTV_GUANGYA_CLIENT_ID" => Ok("from-environment".into()),
            "TTV_GUANGYA_SCOPE" => Ok("".into()),
            _ => Err(()),
        });

        assert_eq!(config.guangya.account_base_url, "https://example.invalid");
        assert_eq!(
            config.guangya.client_id.as_deref(),
            Some("from-environment")
        );
        assert_eq!(config.guangya.scope, None);
    }

    #[test]
    fn missing_config_creates_an_editable_template() {
        let data_dir =
            std::env::temp_dir().join(format!("ttv-config-test-{}", uuid::Uuid::new_v4()));
        let config = AppConfig::load(&data_dir).unwrap();
        let config_path = data_dir.join(CONFIG_FILE_NAME);

        assert!(config_path.is_file());
        assert!(config.oauth.contains_key("cloud123"));
        let contents = std::fs::read_to_string(&config_path).unwrap();
        assert!(contents.contains("\"cloud123\""));
        assert!(contents.contains("\"clientId\": null"));

        let _ = std::fs::remove_dir_all(data_dir);
    }
}
