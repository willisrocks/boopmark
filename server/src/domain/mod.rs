pub mod bookmark;
pub mod chunking;
pub mod error;
pub mod invite;
pub mod llm_settings;
pub mod ports;
pub mod search;
// The merge, proposal and undo API is called by the /tags page and the review
// screen (tag spec rollout steps 2 and 4); until then only the write path uses
// it. The same applies to the tag_alias modules in ports, adapters and app.
#[allow(dead_code)]
pub mod tag_alias;
pub mod tag_consolidation;
pub mod tag_library;
#[allow(dead_code)]
pub mod tag_normalize;
pub mod transfer;
pub mod user;
