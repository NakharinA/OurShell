#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => ourshell_server::run().await,
        Some("healthcheck") => ourshell_server::healthcheck().await,
        Some("set-admin-password") => ourshell_server::set_admin_password(),
        Some(other) => {
            eprintln!(
                "usage: ourshell-server [serve | healthcheck | set-admin-password]
                 unknown command `{other}`"
            );
            std::process::exit(2);
        }
    }
}
