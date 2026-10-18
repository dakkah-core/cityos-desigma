//! CityOS desktop capabilities shared by application adapters.
pub mod jobs;
pub use jobs as runtime;
pub mod bridge;
pub mod host;
pub mod layout;
pub mod repaint;
pub mod dependencies;
pub mod projection;
pub mod draft;
pub mod editor;
pub mod history;
pub mod storage_policy;
pub mod workspace;
#[doc(hidden)]
pub mod workspace_support;
pub mod catalog;
#[cfg(test)]
mod workspace_tests;
