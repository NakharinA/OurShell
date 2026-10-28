//! End-to-end: the real `ourshell` binary (and the agent it spawns) against an
//! in-process SSH server.

#[path = "../../core/tests/common/mod.rs"]
mod common;

use std::process::{Command, Output, Stdio};

struct Env {
    dir: tempfile::TempDir,
    socket: String,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let socket = format!(
            "ourshell-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        );
        Self { dir, socket }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ourshell"));
        c.args(args)
            .env("OURSHELL_VAULT", self.dir.path().join("vault.db"))
            .env("OURSHELL_AGENT_SOCKET", &self.socket)
            .env("OURSHELL_PASSWORD", "correct horse battery")
            .env("OURSHELL_HOST_PASSWORD", "pw")
            .env("OURSHELL_BIN", env!("CARGO_BIN_EXE_ourshell"))
            .current_dir(self.dir.path());
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_stdin(args, "")
    }

    fn run_stdin(&self, args: &[&str], stdin: &str) -> Output {
        let mut child = self
            .cmd(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        self.ok_stdin(args, "")
    }

    fn ok_stdin(&self, args: &[&str], stdin: &str) -> String {
        let out = self.run_stdin(args, stdin);
        assert!(
            out.status.success(),
            "`ourshell {}` failed:\nstdout: {}\nstderr: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn path(&self, p: &str) -> String {
        self.dir.path().join(p).display().to_string()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.run(&["vault", "stop-agent"]);
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn vault_hosts_exec_sftp_keys_import() {
    let server = common::TestServer::start(None).await;
    let port = server.addr.port();
    let sftp_root = server.sftp_root.path().to_path_buf();

    blocking(move || {
        let e = Env::new();

        // Vault lifecycle.
        let out = e.ok(&["vault", "init"]);
        assert!(out.contains("RECOVERY PHRASE"));
        assert!(e.run(&["vault", "init"]).status.code() != Some(0));
        e.ok(&["vault", "lock"]);
        assert!(e.ok(&["vault", "status"]).contains("locked"));

        // Hosts and identities (unlocks automatically via $OURSHELL_PASSWORD).
        e.ok(&["identity", "add", "tester", "-u", "test"]);
        e.ok(&["host", "add", "box", &format!("127.0.0.1:{port}"), "--identity", "tester", "--tags", "ci"]);
        let ls = e.ok(&["host", "ls"]);
        assert!(ls.contains("box") && ls.contains("127.0.0.1"));
        // Secrets never leave the agent.
        let shown = e.ok(&["--json", "identity", "ls"]);
        assert!(!shown.contains("\"pw\""));

        // First connection: unknown host key is refused unless the user says yes.
        let refused = e.run_stdin(&["exec", "box", "echo", "hi"], "no\n");
        assert!(!refused.status.success());
        let out = e.ok_stdin(&["exec", "box", "echo", "hello-from-ssh"], "yes\n");
        assert_eq!(out.trim(), "hello-from-ssh");
        // Now trusted: no prompt needed.
        assert_eq!(e.ok(&["exec", "box", "echo", "again"]).trim(), "again");

        // SFTP round trip.
        std::fs::write(e.path("up.txt"), b"sftp payload").unwrap();
        e.ok(&["sftp", "put", "box", &e.path("up.txt"), "up.txt"]);
        assert_eq!(std::fs::read(sftp_root.join("up.txt")).unwrap(), b"sftp payload");
        assert!(e.ok(&["sftp", "ls", "box", "."]).contains("up.txt"));
        e.ok(&["sftp", "get", "box", "up.txt", &e.path("down.txt")]);
        assert_eq!(std::fs::read(e.path("down.txt")).unwrap(), b"sftp payload");

        // Keys.
        let gen = e.ok(&["key", "gen", "laptop"]);
        assert!(gen.contains("ssh-ed25519"));
        assert!(e.ok(&["key", "pub", "laptop"]).starts_with("ssh-ed25519 "));

        // ssh_config import.
        std::fs::write(
            e.path("ssh_config"),
            "Host web1\n  HostName 10.1.1.1\n  User deploy\n  Port 2200\nHost box\n  HostName 1.2.3.4\n",
        )
        .unwrap();
        let out = e.ok(&["import", "ssh-config", &e.path("ssh_config")]);
        assert!(out.contains("Imported 1 hosts"), "{out}");
        assert!(out.contains("Skipped (already exist): box"), "{out}");
        assert!(e.ok(&["host", "ls"]).contains("web1"));

        // Edit and remove.
        e.ok(&["host", "edit", "web1", "--port", "2201"]);
        assert!(e.ok(&["host", "show", "web1"]).contains("2201"));
        e.ok(&["host", "rm", "web1"]);
        assert!(!e.ok(&["host", "ls"]).contains("web1"));
    })
    .await;
}
