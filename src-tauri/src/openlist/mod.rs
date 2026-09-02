//! OpenList sidecar runtime and API adapter.
//!
//! The desktop client talks to OpenList only through this module.  Credentials
//! remain inside OpenList (or the platform credential store) and are never
//! returned to the webview.

mod client;
mod runtime;

pub use client::{
    OpenListAccountInfo, OpenListClient, OpenListField, OpenListFile, OpenListFilePage,
    OpenListStorage, OpenListStorageInput, OpenListStorageSchema, StorageQuota,
};
pub use runtime::{OpenListRuntime, OpenListRuntimeStatus};
