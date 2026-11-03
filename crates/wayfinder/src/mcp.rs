//! MCP over stdio: the same pool operations, exposed as tools a harness calls.

use anyhow::{Result, bail};
use wayfinder_core::Store;

/// Serve the pool on stdin/stdout until the client closes the pipe.
/// @param store - the pool this server answers about
pub fn serve(_store: Store) -> Result<()> {
    bail!("`wayfinder mcp` lands in the next milestone")
}
