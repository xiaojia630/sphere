//! The web playground has no sphere-host: no shells to start or re-attach (its tabs are scripted, see `term`).
use std::sync::mpsc;

pub const LABEL: &str = "sphere-term";

/// What sphere-host says about a shell it holds; the playground never has one.
#[derive(Clone, Debug)]
pub struct ChildInfo {
    pub cmd: String,
}

pub struct HostWriter;

impl std::io::Write for HostWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct Handle {
    pub nonce: Option<String>,
}

impl Handle {
    pub fn kill(&self) {}
}

pub fn running() -> Vec<ChildInfo> {
    Vec::new()
}

pub fn spawn(_cols: u16, _rows: u16, _shell: &str) -> Option<(Handle, mpsc::Receiver<Vec<u8>>, HostWriter)> {
    None
}

pub fn replay_len(_ch: &ChildInfo) -> u64 {
    0
}

pub fn attach(_ch: &ChildInfo) -> Option<(Handle, mpsc::Receiver<Vec<u8>>, HostWriter)> {
    None
}
