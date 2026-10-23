//! CityOS desktop capabilities shared by application adapters.
pub mod jobs;
pub use jobs as runtime;
pub mod bridge;
pub mod catalog;
pub mod dependencies;
pub mod draft;
pub mod editor;
pub mod history;
pub mod host;
pub mod layout;
pub mod projection;
pub mod repaint;
pub mod storage_policy;
mod submission_policy;
pub mod workspace;
#[doc(hidden)]
pub mod workspace_support;
#[cfg(test)]
mod workspace_tests;
