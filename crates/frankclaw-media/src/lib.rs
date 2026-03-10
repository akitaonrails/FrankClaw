#![forbid(unsafe_code)]

mod fetch;
mod store;

pub use fetch::SafeFetcher;
pub use store::MediaStore;
