mod file_storage;
// Private, and re-exported below as `Storage`, so the repeated name never shows in a path.
#[allow(clippy::module_inception)]
mod storage;

pub(crate) mod accessory_database;

pub use self::{file_storage::FileStorage, storage::Storage};
