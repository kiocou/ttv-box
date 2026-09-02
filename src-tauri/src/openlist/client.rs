use std::collections::BTreeMap;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageQuota {
    pub total: Option<u64>,
    pub used: Option<u64>,
    pub free: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListStorage {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub mount_path: String,
    pub enabled: bool,
    pub connection: String,
    pub account_name: Option<String>,
    pub account_id: Option<String>,
    pub quota: Option<StorageQuota>,
    pub last_sync_at: Option<i64>,
    pub file_count: Option<u64>,
    pub icon_asset: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListField {
    pub key: String,
    pub label: String,
    pub kind: String,
    pub required: bool,
    pub secret: bool,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListStorageSchema {
    pub driver: String,
    pub fields: Vec<OpenListField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListStorageInput {
    pub id: Option<String>,
    pub name: String,
    pub driver: String,
    pub mount_path: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub fields: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListFile {
    pub id: String,
    pub name: String,
    pub path: String,
    pub is_folder: bool,
    pub size: Option<u64>,
    pub modified_at: Option<i64>,
    pub mime_type: Option<String>,
    pub thumbnail_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListFilePage {
    pub storage_id: String,
    pub path: String,
    pub files: Vec<OpenListFile>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListAccountInfo {
    pub storage_id: String,
    pub account_id: Option<String>,
    pub account_name: Option<String>,
    pub quota: Option<StorageQuota>,
}

#[derive(Debug, Clone)]
pub struct OpenListClient {
    http: Client,
    base_url: String,
    token: Option<String>,
}

impl OpenListClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self, AppError> {
        Ok(Self {
            http: Client::builder()
                .user_agent("TTV-Box/OpenList-bridge")
                .build()
                .map_err(|error| AppError::Runtime(error.to_string()))?,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            token: None,
        })
    }

    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token.filter(|value| !value.trim().is_empty());
        self
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<String, AppError> {
        let response = self
            .http
            .post(format!("{}/api/auth/login", self.base_url))
            .json(&json!({"username": username, "password": password}))
            .send()
            .await
            .map_err(|error| AppError::Provider(format!("OpenList 登录网络错误：{error}")))?;
        let status = response.status();
        let body = response.json::<Value>().await.unwrap_or_else(|_| json!({}));
        if !status.is_success() {
            return Err(AppError::Provider(format!(
                "OpenList 登录失败（HTTP {}）",
                status.as_u16()
            )));
        }
        value_string(
            body.get("data").unwrap_or(&body),
            &["token", "access_token", "accessToken"],
        )
        .ok_or_else(|| AppError::Provider("OpenList 登录响应未包含会话令牌".into()))
    }

    pub async fn health(&self) -> Result<(bool, Option<String>), AppError> {
        let response = self
            .http
            .get(format!("{}/api/public/settings", self.base_url))
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => {
                let body = response.json::<Value>().await.unwrap_or_default();
                let version = body
                    .pointer("/data/version")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        body.get("version")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    });
                Ok((true, version))
            }
            Ok(_) => Ok((false, None)),
            Err(_) => Ok((false, None)),
        }
    }

    pub async fn account_info(&self, storage_id: &str) -> Result<OpenListAccountInfo, AppError> {
        let body = self
            .request_json(self.http.get(format!(
                "{}/api/admin/storage/get?id={}",
                self.base_url,
                urlencoding(storage_id)
            )))
            .await?;
        let data = body.get("data").unwrap_or(&body);
        Ok(OpenListAccountInfo {
            storage_id: storage_id.to_owned(),
            account_id: value_string(data, &["account_id", "accountId", "id"]),
            account_name: value_string(data, &["account_name", "accountName", "username", "name"]),
            quota: quota_from(data),
        })
    }

    pub async fn storage_list(&self) -> Result<Vec<OpenListStorage>, AppError> {
        let body = self
            .request_json(
                self.http
                    .get(format!("{}/api/admin/storage/list", self.base_url)),
            )
            .await?;
        let values = body
            .pointer("/data/content")
            .or_else(|| body.pointer("/data"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(values.iter().map(storage_from).collect())
    }

    pub async fn storage_schema(&self, driver: &str) -> Result<OpenListStorageSchema, AppError> {
        let body = self
            .request_json(
                self.http
                    .get(format!("{}/api/admin/storage/drivers", self.base_url)),
            )
            .await?;
        let drivers = body
            .pointer("/data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let item = drivers.iter().find(|item| {
            value_string(item, &["name", "driver", "key"])
                .as_deref()
                .map(|value| value.eq_ignore_ascii_case(driver))
                .unwrap_or(false)
        });
        let fields = item
            .and_then(|item| item.get("addition").or_else(|| item.get("fields")))
            .and_then(Value::as_array)
            .map(|items| items.iter().map(field_from).collect())
            .unwrap_or_else(|| generic_fields(driver));
        Ok(OpenListStorageSchema {
            driver: driver.to_owned(),
            fields,
        })
    }

    pub async fn storage_save(
        &self,
        input: &OpenListStorageInput,
    ) -> Result<OpenListStorage, AppError> {
        let payload = json!({"id": input.id, "mount_path": input.mount_path, "order": 0, "remark": input.name, "driver": input.driver, "enable": input.enabled, "addition": input.fields});
        let request = if input.id.is_some() {
            self.http
                .post(format!("{}/api/admin/storage/update", self.base_url))
        } else {
            self.http
                .post(format!("{}/api/admin/storage/create", self.base_url))
        };
        let body = self.request_json(request.json(&payload)).await?;
        Ok(storage_from(body.get("data").unwrap_or(&body)))
    }

    pub async fn storage_delete(&self, id: &str) -> Result<bool, AppError> {
        let body = self
            .request_json(
                self.http
                    .post(format!("{}/api/admin/storage/delete", self.base_url))
                    .json(&json!({"id": id})),
            )
            .await?;
        Ok(body.get("code").and_then(Value::as_i64).unwrap_or(200) == 200)
    }

    pub async fn storage_test(&self, id: &str) -> Result<OpenListStorage, AppError> {
        let body = self
            .request_json(
                self.http
                    .post(format!("{}/api/admin/storage/test", self.base_url))
                    .json(&json!({"id": id})),
            )
            .await?;
        Ok(storage_from(body.get("data").unwrap_or(&body)))
    }

    pub async fn list_files(
        &self,
        storage_id: &str,
        path: &str,
        page_size: u32,
        cursor: Option<&str>,
        query: Option<&str>,
    ) -> Result<OpenListFilePage, AppError> {
        let body = self.request_json(self.http.post(format!("{}/api/fs/list", self.base_url)).json(&json!({"path": path, "page": cursor.unwrap_or("1").parse::<u32>().unwrap_or(1), "per_page": page_size.min(500), "refresh": false, "password": ""}))).await?;
        let data = body.get("data").unwrap_or(&body);
        let mut files = data
            .get("content")
            .or_else(|| data.get("files"))
            .and_then(Value::as_array)
            .map(|items| items.iter().map(file_from).collect::<Vec<_>>())
            .unwrap_or_default();
        if let Some(query) = query.filter(|value| !value.trim().is_empty()) {
            let needle = query.to_ascii_lowercase();
            files.retain(|item| item.name.to_ascii_lowercase().contains(&needle));
        }
        let next = data
            .get("next")
            .or_else(|| data.get("nextPage"))
            .and_then(Value::as_u64)
            .map(|value| value.to_string());
        Ok(OpenListFilePage {
            storage_id: storage_id.to_owned(),
            path: path.to_owned(),
            has_more: next.is_some(),
            next_cursor: next,
            files,
        })
    }

    pub async fn resolve_playback(&self, path: &str) -> Result<String, AppError> {
        let body = self
            .request_json(
                self.http
                    .post(format!("{}/api/fs/get", self.base_url))
                    .json(&json!({"path": path, "password": ""})),
            )
            .await?;
        let data = body.get("data").unwrap_or(&body);
        value_string(
            data,
            &["raw_url", "rawUrl", "url", "download_url", "downloadUrl"],
        )
        .ok_or_else(|| AppError::NotFound("OpenList 未返回可播放地址".into()))
    }

    async fn request_json(&self, request: reqwest::RequestBuilder) -> Result<Value, AppError> {
        let request = if let Some(token) = self.token.as_deref() {
            request.header("Authorization", token)
        } else {
            request
        };
        let response = request
            .send()
            .await
            .map_err(|error| AppError::Provider(format!("OpenList 网络错误：{error}")))?;
        let status = response.status();
        let body = response.json::<Value>().await.unwrap_or_else(|_| json!({}));
        if !status.is_success()
            || body
                .get("code")
                .and_then(Value::as_i64)
                .is_some_and(|code| code != 200)
        {
            return Err(AppError::Provider(format!(
                "OpenList 请求失败（HTTP {}）",
                status.as_u16()
            )));
        }
        Ok(body)
    }
}

fn storage_from(value: &Value) -> OpenListStorage {
    let id =
        value_string(value, &["id", "key"]).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let name = value_string(value, &["remark", "name", "mount_path", "mountPath"])
        .unwrap_or_else(|| id.clone());
    let driver = value_string(value, &["driver", "driverName"]).unwrap_or_else(|| "unknown".into());
    let mount_path =
        value_string(value, &["mount_path", "mountPath"]).unwrap_or_else(|| "/".into());
    OpenListStorage {
        id,
        name,
        driver,
        mount_path,
        enabled: value
            .get("enable")
            .and_then(Value::as_bool)
            .or_else(|| value.get("enabled").and_then(Value::as_bool))
            .unwrap_or(true),
        connection: "unknown".into(),
        account_name: value_string(value, &["username", "accountName", "account_name"]),
        account_id: value_string(value, &["accountId", "account_id"]),
        quota: quota_from(value),
        last_sync_at: None,
        file_count: None,
        icon_asset: None,
    }
}

fn file_from(value: &Value) -> OpenListFile {
    let name = value_string(value, &["name", "title"]).unwrap_or_else(|| "未命名".into());
    let path = value_string(value, &["path"]).unwrap_or_else(|| name.clone());
    let is_folder = value
        .get("is_dir")
        .and_then(Value::as_bool)
        .or_else(|| value.get("isFolder").and_then(Value::as_bool))
        .or_else(|| {
            value
                .get("type")
                .and_then(Value::as_str)
                .map(|kind| kind == "folder")
        })
        .unwrap_or(false);
    OpenListFile {
        id: path.clone(),
        name,
        path,
        is_folder,
        size: value.get("size").and_then(Value::as_u64),
        modified_at: value
            .get("modified")
            .and_then(Value::as_i64)
            .or_else(|| value.get("modifiedAt").and_then(Value::as_i64)),
        mime_type: value_string(value, &["mime", "mimeType"]),
        thumbnail_url: value_string(value, &["thumbnail", "thumbnailUrl"]),
    }
}

fn field_from(value: &Value) -> OpenListField {
    let key = value_string(value, &["key", "name"]).unwrap_or_else(|| "value".into());
    OpenListField {
        label: value_string(value, &["label", "name", "title"]).unwrap_or_else(|| key.clone()),
        kind: value_string(value, &["type", "kind"]).unwrap_or_else(|| "text".into()),
        required: value
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        secret: value
            .get("secret")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                key.to_ascii_lowercase().contains("token")
                    || key.to_ascii_lowercase().contains("password")
            }),
        options: value
            .get("options")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        key,
    }
}

fn generic_fields(driver: &str) -> Vec<OpenListField> {
    vec![
        OpenListField {
            key: "username".into(),
            label: "账号 / Client ID".into(),
            kind: "text".into(),
            required: false,
            secret: false,
            options: Vec::new(),
        },
        OpenListField {
            key: "password".into(),
            label: "密码 / Token".into(),
            kind: "password".into(),
            required: false,
            secret: true,
            options: Vec::new(),
        },
        OpenListField {
            key: "root_folder_id".into(),
            label: format!("{driver} 根目录"),
            kind: "text".into(),
            required: false,
            secret: false,
            options: Vec::new(),
        },
    ]
}

fn quota_from(value: &Value) -> Option<StorageQuota> {
    let quota = value.get("quota").unwrap_or(value);
    let total = quota.get("total").and_then(Value::as_u64);
    let used = quota.get("used").and_then(Value::as_u64);
    let free = quota.get("free").and_then(Value::as_u64);
    (total.is_some() || used.is_some() || free.is_some()).then_some(StorageQuota {
        total,
        used,
        free,
    })
}

fn value_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str).map(str::to_owned))
}

fn urlencoding(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('/', "%2F")
        .replace('?', "%3F")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_mapping_handles_openlist_shape() {
        let file =
            file_from(&json!({"name":"movie.mkv","path":"/movie.mkv","is_dir":false,"size":12}));
        assert_eq!(file.name, "movie.mkv");
        assert!(!file.is_folder);
    }
}
