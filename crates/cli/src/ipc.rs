//! Local socket naming for the agent (named pipe on Windows, Unix socket elsewhere).

use std::io;

use interprocess::local_socket::{GenericFilePath, GenericNamespaced, Name, ToFsName, ToNsName};

/// Env var overriding the socket name (used by tests and multi-profile setups).
pub const SOCKET_ENV: &str = "OURSHELL_AGENT_SOCKET";

fn username() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".into())
}

/// The human-readable socket identifier.
pub fn socket_id() -> String {
    if let Ok(s) = std::env::var(SOCKET_ENV) {
        return s;
    }
    if cfg!(windows) {
        format!("ourshell-agent-{}", username())
    } else {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(|d| std::path::PathBuf::from(d).join("ourshell"))
            .unwrap_or_else(|| home().join(".ourshell"));
        dir.join("agent.sock").to_string_lossy().into_owned()
    }
}

pub fn home() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| ".".into())
}

/// Resolves [`socket_id`] into an interprocess name.
pub fn socket_name(id: &str) -> io::Result<Name<'static>> {
    if cfg!(windows) || !id.contains('/') {
        id.to_owned().to_ns_name::<GenericNamespaced>()
    } else {
        if let Some(dir) = std::path::Path::new(id).parent() {
            std::fs::create_dir_all(dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        id.to_owned().to_fs_name::<GenericFilePath>()
    }
}
