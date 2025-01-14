//! Azure Storage Singer

mod signer;
pub use signer::Signer as AzureStorageSigner;
mod config;
pub use config::Config as AzureStorageConfig;
mod credential;
pub use credential::Credential as AzureStorageCredential;
mod loader;
pub use loader::Loader as AzureStorageLoader;
