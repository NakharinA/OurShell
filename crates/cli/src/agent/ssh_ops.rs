//! SSH operations used by the agent (stub until the SSH client lands).

use std::sync::Arc;

use ourshell_proto::agent::{DirEntry, FileData};
use ourshell_proto::rpc::{self, RpcError};

use super::{err, Conn, State};

pub struct ExecOut {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<u32>,
    pub truncated: bool,
}

fn unsupported() -> RpcError {
    err(rpc::INTERNAL_ERROR, "SSH is not available in this build")
}

pub async fn exec(
    _state: &Arc<State>,
    _host_id: &str,
    _for_ai: bool,
    _command: &str,
    _timeout_secs: Option<u64>,
    _max_output: usize,
) -> Result<ExecOut, RpcError> {
    Err(unsupported())
}

pub async fn sftp_list(
    _state: &Arc<State>,
    _host_id: &str,
    _for_ai: bool,
    _path: &str,
) -> Result<Vec<DirEntry>, RpcError> {
    Err(unsupported())
}

pub async fn sftp_read(
    _state: &Arc<State>,
    _host_id: &str,
    _for_ai: bool,
    _path: &str,
    _max: usize,
) -> Result<FileData, RpcError> {
    Err(unsupported())
}

pub async fn sftp_write(
    _state: &Arc<State>,
    _host_id: &str,
    _for_ai: bool,
    _path: &str,
    _data: &[u8],
) -> Result<(), RpcError> {
    Err(unsupported())
}

pub async fn connection_closed(_state: &Arc<State>, _conn: &Arc<Conn>) {}
