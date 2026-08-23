//! sluice-bridge — the AI-agent side of Sluice (03 §2/§3). This first slice is
//! the built-in **read-only MCP server** over stdio: any MCP client (Claude
//! Code, Codex CLI, Gemini CLI, …) can read repository state through four
//! tools. Write *proposals*, hooks and the desktop-instance IPC arrive with
//! the rest of M4; destructive operations will never get a tool (03 §3).

pub mod mcp;
