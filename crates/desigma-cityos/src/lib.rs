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
pub mod catalog;
